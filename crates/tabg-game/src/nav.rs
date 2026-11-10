//! The bots' map: a top-surface heightfield (from the dumper's walkability grid). Bots walk on
//! it, path around anything they can't step up (buildings, cliffs, water) and use it for line
//! of sight. Gateways the heightfield can't see (a passage under an arch looks like a wall from
//! above) are added as links, and flow fields give the way to one goal from anywhere around it.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::rng::Rng;

/// Highest step a bot takes between neighbouring cells (metres per cell).
const MAX_STEP: f32 = 0.7;
/// Ground at least this far above a walker is a roof or an arch they pass under.
const HEAD_ROOM: f32 = 2.5;
/// Eye height above the feet.
pub const EYE: f32 = 1.6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavGrid {
    pub min_x: f32,
    pub min_z: f32,
    pub cell: f32,
    pub width: usize,
    pub depth: usize,
    /// `(height - 100) * 64`, 0 = water / nothing.
    pub heights: Vec<u16>,
    /// Gateways: cell → cells reachable straight from it (and the cost), both ways.
    pub links: HashMap<usize, Vec<(usize, u32)>>,
}

/// Distances (in path cost, 10 per metre) from every cell of a region to one goal, so walkers
/// anywhere in it can head there without a search of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowField {
    pub goal: [f32; 3],
    x0: usize,
    z0: usize,
    width: usize,
    depth: usize,
    dist: Vec<u32>,
}

impl FlowField {
    fn local(&self, cx: usize, cz: usize) -> Option<usize> {
        let (x, z) = (cx.checked_sub(self.x0)?, cz.checked_sub(self.z0)?);
        (x < self.width && z < self.depth).then_some(z * self.width + x)
    }

    fn cost(&self, cx: usize, cz: usize) -> Option<u32> {
        self.local(cx, cz)
            .map(|i| self.dist[i])
            .filter(|&d| d != u32::MAX)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Node {
    cost: u32,
    index: usize,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .cmp(&self.cost)
            .then(self.index.cmp(&other.index))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl NavGrid {
    fn cell_of(&self, x: f32, z: f32) -> Option<(usize, usize)> {
        let cx = ((x - self.min_x) / self.cell).floor();
        let cz = ((z - self.min_z) / self.cell).floor();
        if cx < 0.0 || cz < 0.0 {
            return None;
        }
        let (cx, cz) = (cx as usize, cz as usize);
        (cx < self.width && cz < self.depth).then_some((cx, cz))
    }

    fn raw(&self, cx: usize, cz: usize) -> u16 {
        self.heights[cz * self.width + cx]
    }

    fn height_of_cell(&self, cx: usize, cz: usize) -> Option<f32> {
        let v = self.raw(cx, cz);
        (v != 0).then(|| 100.0 + f32::from(v) / 64.0)
    }

    fn centre(&self, cx: usize, cz: usize) -> [f32; 3] {
        let x = self.min_x + (cx as f32 + 0.5) * self.cell;
        let z = self.min_z + (cz as f32 + 0.5) * self.cell;
        [x, self.height_of_cell(cx, cz).unwrap_or(100.0), z]
    }

    /// Ground (top surface) height at `x, z`; `None` over water or off the map.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let (cx, cz) = self.cell_of(x, z)?;
        self.height_of_cell(cx, cz)
    }

    /// Can a bot stand here (land, and not a lone spike)?
    pub fn walkable(&self, x: f32, z: f32) -> bool {
        self.height(x, z).is_some()
    }

    /// Let walkers pass straight between `a` and `b` where the heightfield shows a wall (a
    /// gateway under an arch). Both ends must be walkable; returns whether the link was added.
    pub fn add_link(&mut self, a: [f32; 3], b: [f32; 3]) -> bool {
        let (Some(ca), Some(cb)) = (self.cell_of(a[0], a[2]), self.cell_of(b[0], b[2])) else {
            return false;
        };
        if self.height_of_cell(ca.0, ca.1).is_none() || self.height_of_cell(cb.0, cb.1).is_none() {
            return false;
        }
        // Grid edges cost 10 per cell.
        let cost = ((a[0] - b[0]).hypot(a[2] - b[2]) / self.cell * 10.0).max(10.0) as u32;
        let (ia, ib) = (ca.1 * self.width + ca.0, cb.1 * self.width + cb.0);
        self.links.entry(ia).or_default().push((ib, cost));
        self.links.entry(ib).or_default().push((ia, cost));
        true
    }

    /// Height a walker coming from height `from_y` stands at on `x, z`: the ground, except
    /// under an arch or overhang (with room to stand under it), where they stay on their level.
    /// Ground that only rises (a gateway's far end higher than its near one) is stepped onto,
    /// or the walker would stay buried in it.
    pub fn walk_height(&self, from_y: f32, x: f32, z: f32) -> f32 {
        match self.height(x, z) {
            Some(h) if h <= from_y + HEAD_ROOM => h,
            _ => from_y,
        }
    }

    /// Neighbours with their cost, gateway links included.
    fn neighbours_linked(
        &self,
        cx: usize,
        cz: usize,
    ) -> impl Iterator<Item = (usize, usize, u32)> + '_ {
        let links = self
            .links
            .get(&(cz * self.width + cx))
            .into_iter()
            .flatten()
            .map(|&(i, cost)| (i % self.width, i / self.width, cost));
        self.neighbours(cx, cz).chain(links)
    }

    /// Path costs to `goal` from every cell between `min` and `max` (`[x, z]`) that can walk
    /// there. `None` when the goal isn't on walkable ground.
    pub fn flow_field(&self, goal: [f32; 3], min: [f32; 2], max: [f32; 2]) -> Option<FlowField> {
        let g = self.cell_of(goal[0], goal[2])?;
        self.height_of_cell(g.0, g.1)?;
        let lo = (
            self.cell_of(min[0].max(self.min_x), min[1].max(self.min_z))?,
            self.cell_of(
                max[0].min(self.min_x + (self.width as f32 - 1.0) * self.cell),
                max[1].min(self.min_z + (self.depth as f32 - 1.0) * self.cell),
            )?,
        );
        let (x0, z0) = (lo.0.0.min(g.0), lo.0.1.min(g.1));
        let (x1, z1) = (lo.1.0.max(g.0), lo.1.1.max(g.1));
        let mut field = FlowField {
            goal,
            x0,
            z0,
            width: x1 - x0 + 1,
            depth: z1 - z0 + 1,
            dist: Vec::new(),
        };
        field.dist = vec![u32::MAX; field.width * field.depth];
        let start = field.local(g.0, g.1).expect("goal is inside");
        field.dist[start] = 0;
        let mut open = BinaryHeap::new();
        open.push(Node {
            cost: 0,
            index: g.1 * self.width + g.0,
        });
        while let Some(Node { cost, index }) = open.pop() {
            let (cx, cz) = (index % self.width, index / self.width);
            let Some(here) = field.local(cx, cz) else {
                continue;
            };
            if cost > field.dist[here] {
                continue;
            }
            for (nx, nz, step) in self.neighbours_linked(cx, cz) {
                let Some(there) = field.local(nx, nz) else {
                    continue;
                };
                let candidate = cost + step;
                if candidate < field.dist[there] {
                    field.dist[there] = candidate;
                    open.push(Node {
                        cost: candidate,
                        index: nz * self.width + nx,
                    });
                }
            }
        }
        Some(field)
    }

    /// Metres to walk from `at` to the field's goal; `None` outside the field or cut off.
    pub fn flow_distance(&self, field: &FlowField, at: [f32; 3]) -> Option<f32> {
        let (cx, cz) = self.cell_of(at[0], at[2])?;
        field.cost(cx, cz).map(|c| c as f32 / 10.0 * self.cell)
    }

    /// Where to walk next from `at` towards the field's goal: up to `ahead` cells down the
    /// field (stopping at a gateway, which is walked straight through). `None` outside the
    /// field; the goal itself once there.
    pub fn flow_step(&self, field: &FlowField, at: [f32; 3], ahead: usize) -> Option<[f32; 3]> {
        self.flow_next(field, at, ahead).map(|(p, _)| p)
    }

    /// Like [`NavGrid::flow_step`], and whether the step goes through a gateway (walk it to the
    /// end before asking again: under the arch the field has no cells).
    pub fn flow_next(
        &self,
        field: &FlowField,
        at: [f32; 3],
        ahead: usize,
    ) -> Option<([f32; 3], bool)> {
        let (mut cx, mut cz) = self.cell_of(at[0], at[2])?;
        let Some(mut best) = field.cost(cx, cz) else {
            // Walking straight between cells can end up on one the field skips (a post, a
            // crate's edge): back onto the field, to the best cell within 3 cells.
            return self.nearest_on_field(field, cx, cz, 3).map(|p| (p, false));
        };
        let mut gateway = false;
        for step in 0..ahead.max(1) {
            let here = cz * self.width + cx;
            let next = self
                .neighbours_linked(cx, cz)
                .filter_map(|(nx, nz, _)| field.cost(nx, nz).map(|c| (c, nx, nz)))
                .min();
            match next {
                Some((c, nx, nz)) if c < best => {
                    let linked = self
                        .links
                        .get(&here)
                        .is_some_and(|l| l.iter().any(|&(i, _)| i == nz * self.width + nx));
                    // A gateway is only entered from its own end, or the straight line there
                    // would cut through the wall beside it.
                    if linked && step > 0 {
                        break;
                    }
                    (cx, cz, best) = (nx, nz, c);
                    if linked {
                        gateway = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        Some((self.centre(cx, cz), gateway))
    }

    fn nearest_on_field(
        &self,
        field: &FlowField,
        cx: usize,
        cz: usize,
        reach: i32,
    ) -> Option<[f32; 3]> {
        let mut best: Option<(i32, u32, usize, usize)> = None;
        for dz in -reach..=reach {
            for dx in -reach..=reach {
                let (Ok(x), Ok(z)) = (
                    usize::try_from(cx as i32 + dx),
                    usize::try_from(cz as i32 + dz),
                ) else {
                    continue;
                };
                if let Some(c) = field.cost(x, z) {
                    let key = (dx * dx + dz * dz, c, x, z);
                    if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                        best = Some(key);
                    }
                }
            }
        }
        best.map(|(_, _, x, z)| self.centre(x, z))
    }

    fn neighbours(&self, cx: usize, cz: usize) -> impl Iterator<Item = (usize, usize, u32)> + '_ {
        const DIRS: [(i32, i32, u32); 8] = [
            (1, 0, 10),
            (-1, 0, 10),
            (0, 1, 10),
            (0, -1, 10),
            (1, 1, 14),
            (1, -1, 14),
            (-1, 1, 14),
            (-1, -1, 14),
        ];
        let here = self.height_of_cell(cx, cz);
        DIRS.into_iter().filter_map(move |(dx, dz, cost)| {
            let nx = usize::try_from(cx as i32 + dx).ok()?;
            let nz = usize::try_from(cz as i32 + dz).ok()?;
            if nx >= self.width || nz >= self.depth {
                return None;
            }
            let there = self.height_of_cell(nx, nz)?;
            let step = (there - here?).abs();
            let limit = if dx != 0 && dz != 0 {
                MAX_STEP * 1.41
            } else {
                MAX_STEP
            };
            (step <= limit).then_some((nx, nz, cost + (step * 20.0) as u32))
        })
    }

    /// A* from `from` to `to` over walkable cells, visiting at most `budget` cells. Returns
    /// waypoints (every few metres) ending at `to`, or `None` if it's unreachable in budget.
    pub fn path(&self, from: [f32; 3], to: [f32; 3], budget: usize) -> Option<Vec<[f32; 3]>> {
        let start = self.cell_of(from[0], from[2])?;
        let goal = self.cell_of(to[0], to[2])?;
        self.height_of_cell(goal.0, goal.1)?;
        let index = |(x, z): (usize, usize)| z * self.width + x;
        let heuristic = |(x, z): (usize, usize)| {
            let dx = x.abs_diff(goal.0) as u32;
            let dz = z.abs_diff(goal.1) as u32;
            10 * dx.max(dz) + 4 * dx.min(dz)
        };
        let mut open = BinaryHeap::new();
        let mut came: HashMap<usize, usize> = HashMap::new();
        let mut cost: HashMap<usize, u32> = HashMap::new();
        open.push(Node {
            cost: heuristic(start),
            index: index(start),
        });
        cost.insert(index(start), 0);
        let mut visited = 0;
        while let Some(Node { index: current, .. }) = open.pop() {
            let (cx, cz) = (current % self.width, current / self.width);
            if (cx, cz) == goal {
                let mut cells = vec![current];
                let mut at = current;
                while let Some(&prev) = came.get(&at) {
                    cells.push(prev);
                    at = prev;
                }
                cells.reverse();
                // Waypoints where the path turns (straight runs between them stay on the path).
                let xy = |c: usize| ((c % self.width) as i64, (c / self.width) as i64);
                let mut points = Vec::new();
                for w in cells.windows(3) {
                    let (a, b, c) = (xy(w[0]), xy(w[1]), xy(w[2]));
                    if (b.0 - a.0, b.1 - a.1) != (c.0 - b.0, c.1 - b.1) {
                        points.push(self.centre(b.0 as usize, b.1 as usize));
                    }
                }
                points.push(self.centre(goal.0, goal.1));
                return Some(points);
            }
            visited += 1;
            if visited > budget {
                return None;
            }
            let g = cost[&current];
            for (nx, nz, step) in self.neighbours_linked(cx, cz) {
                let next = index((nx, nz));
                let candidate = g + step;
                if cost.get(&next).is_none_or(|&c| candidate < c) {
                    cost.insert(next, candidate);
                    came.insert(next, current);
                    open.push(Node {
                        cost: candidate + heuristic((nx, nz)),
                        index: next,
                    });
                }
            }
        }
        None
    }

    /// Can someone with eyes at `from` see `to` (both feet positions)? Anything on the grid
    /// higher than the sight line in between blocks it.
    pub fn line_of_sight(&self, from: [f32; 3], to: [f32; 3]) -> bool {
        let (a, b) = (
            [from[0], from[1] + EYE, from[2]],
            [to[0], to[1] + EYE * 0.75, to[2]],
        );
        let dist = (b[0] - a[0]).hypot(b[2] - a[2]);
        let steps = (dist / self.cell).ceil().max(1.0) as usize;
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            let (x, y, z) = (
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            );
            if let Some(h) = self.height(x, z)
                && h > y + 0.2
            {
                return false;
            }
        }
        true
    }

    /// A walkable point within `radius` of `centre` (a few tries), else `None`.
    pub fn random_point_near(
        &self,
        centre: [f32; 3],
        radius: f32,
        rng: &mut Rng,
    ) -> Option<[f32; 3]> {
        for _ in 0..24 {
            let [ox, oz] = rng.inside_unit_circle();
            let (x, z) = (centre[0] + ox * radius, centre[2] + oz * radius);
            if let Some(y) = self.height(x, z) {
                return Some([x, y, z]);
            }
        }
        None
    }

    /// Decode the dumper's file (already gunzipped): `TBNG` v1 header, then u16 heights.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 28 || &bytes[..4] != b"TBNG" {
            return Err("not a tabg walkability grid".into());
        }
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        let f32_at = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        if u32_at(4) != 1 {
            return Err("unknown walkability grid version".into());
        }
        let (width, depth) = (u32_at(20) as usize, u32_at(24) as usize);
        let body = &bytes[28..];
        if body.len() != width * depth * 2 {
            return Err("walkability grid has the wrong size".into());
        }
        Ok(Self {
            min_x: f32_at(8),
            min_z: f32_at(12),
            cell: f32_at(16),
            width,
            depth,
            heights: body
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
            links: HashMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20×20 m of ground at 120 m with a 4 m wall at x = 10 (gap at z 16..18) and water at x < 2.
    fn grid() -> NavGrid {
        let (w, d) = (20, 20);
        let mut heights = vec![(20 * 64) as u16; w * d];
        for z in 0..d {
            for x in 0..w {
                if x < 2 {
                    heights[z * w + x] = 0;
                }
                if x == 10 && !(16..18).contains(&z) {
                    heights[z * w + x] = (24 * 64) as u16;
                }
            }
        }
        NavGrid {
            min_x: 0.0,
            min_z: 0.0,
            cell: 1.0,
            width: w,
            depth: d,
            heights,
            ..Default::default()
        }
    }

    #[test]
    fn heights_and_water() {
        let g = grid();
        assert_eq!(g.height(5.5, 5.5), Some(120.0));
        assert_eq!(g.height(0.5, 5.5), None, "water");
        assert_eq!(g.height(-1.0, 5.0), None, "off the map");
    }

    #[test]
    fn paths_go_through_the_gap() {
        let g = grid();
        let path = g
            .path([5.5, 120.0, 2.5], [15.5, 120.0, 2.5], 10_000)
            .expect("reachable");
        assert!(
            path.iter().any(|p| p[2] > 15.0),
            "detours through the gap: {path:?}"
        );
        assert_eq!(*path.last().unwrap(), [15.5, 120.0, 2.5]);
        assert!(
            g.path([5.5, 120.0, 2.5], [0.5, 120.0, 2.5], 10_000)
                .is_none(),
            "water"
        );
    }

    #[test]
    fn walls_block_sight() {
        let g = grid();
        assert!(!g.line_of_sight([5.5, 120.0, 5.5], [15.5, 120.0, 5.5]));
        assert!(
            g.line_of_sight([5.5, 120.0, 17.0], [15.5, 120.0, 17.0]),
            "through the gap"
        );
    }

    #[test]
    fn gateway_links_open_walls_for_paths_and_fields() {
        let mut g = grid();
        // Close the gap: x = 10 is a wall all the way.
        for z in 16..18 {
            g.heights[z * 20 + 10] = (24 * 64) as u16;
        }
        let (from, to) = ([5.5, 120.0, 5.5], [15.5, 120.0, 5.5]);
        assert!(g.path(from, to, 10_000).is_none(), "walled off");
        assert!(g.add_link([9.5, 120.0, 8.5], [11.5, 120.0, 8.5]));
        let path = g.path(from, to, 10_000).expect("through the gateway");
        assert!(path.iter().any(|p| p[0] == 9.5 && p[2] == 8.5), "{path:?}");
        assert!(
            !g.add_link([0.5, 120.0, 0.5], [3.5, 120.0, 0.5]),
            "water end"
        );

        let field = g
            .flow_field(to, [0.0, 0.0], [20.0, 20.0])
            .expect("goal on land");
        let d = g.flow_distance(&field, from).expect("reachable");
        assert!((10.0..20.0).contains(&d), "{d}");
        // Walking down the field gets there, crossing the gateway in one straight step.
        let mut at = from;
        for _ in 0..40 {
            let next = g.flow_step(&field, at, 3).expect("in the field");
            if next[0] > 10.0 && at[0] < 10.0 {
                assert_eq!(at, [9.5, 120.0, 8.5], "only through the gateway");
            }
            at = next;
        }
        assert_eq!(at, [15.5, 120.0, 5.5]);
        assert_eq!(g.flow_distance(&field, [0.5, 120.0, 0.5]), None, "water");
    }

    #[test]
    fn walkers_stay_on_their_level_under_arches() {
        let mut g = grid();
        g.heights[5 * 20 + 5] = (28 * 64) as u16;
        assert_eq!(g.walk_height(120.0, 5.5, 5.5), 120.0, "under the arch");
        assert_eq!(g.walk_height(120.0, 6.5, 5.5), 120.0);
        assert_eq!(g.walk_height(127.6, 5.5, 5.5), 128.0, "on top of it");
        assert_eq!(g.walk_height(125.0, 6.5, 5.5), 120.0, "down is fine");
    }

    #[test]
    fn decodes_the_dump_format() {
        let mut bytes = b"TBNG".to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for f in [-10.0f32, -20.0, 1.0] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&(64u16 * 30).to_le_bytes());
        let g = NavGrid::decode(&bytes).unwrap();
        assert_eq!(g.height(-8.5, -19.5), Some(130.0));
        assert!(NavGrid::decode(b"nope").is_err());
    }
}
