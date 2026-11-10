using System;
using System.Collections.Generic;
using System.Runtime.CompilerServices;
using BepInEx;
using BepInEx.Configuration;
using HarmonyLib;
using Landfall.Network;
using UnityEngine;

[assembly: InternalsVisibleTo("ClientMods.Tests")]

namespace TabgInstaller.PrincessMode
{
    /// <summary>
    /// Capture the Princess on the player's side: a small HUD with both princesses and the
    /// helicopter progress, short banners ("You can now spawn a helicopter!"), cages you can see
    /// from afar, and the bases' anti-aircraft turrets. Everything comes from the server (event
    /// 239); without this mod the game is the same and the banners arrive as chat lines.
    /// </summary>
    [BepInPlugin("tabginstaller.princessmode", "Princess Mode", "1.0.0")]
    [BepInDependency("tabginstaller.modsettings", BepInDependency.DependencyFlags.SoftDependency)]
    public class PrincessModePlugin : BaseUnityPlugin
    {
        internal static PrincessModePlugin Instance;

        private static readonly Color[] TeamColours = { new Color(0.3f, 0.6f, 1f), new Color(1f, 0.45f, 0.75f) };

        private PrincessState _state;
        private float _stateAt = -100f;
        private Offer _offer;
        private float _offerUntil;
        private readonly List<(Toast toast, float until)> _toasts = new List<(Toast, float)>();
        private readonly List<GameObject> _props = new List<GameObject>();
        private readonly List<Transform> _turretHeads = new List<Transform>();
        private string _propsFor;
        private float _nextHello;

        private ConfigEntry<KeyCode> _toggleKey;
        private ConfigEntry<bool> _hud;
        private ConfigEntry<bool> _labels;
        private Harmony _harmony;
        private Texture2D _white;
        private GUIStyle _title;
        private GUIStyle _line;
        private GUIStyle _banner;
        private GUIStyle _label;

        private void Awake()
        {
            Instance = this;
            _toggleKey = Config.Bind("Princess", "ToggleKey", KeyCode.F3, "Show or hide the Capture the Princess HUD.");
            _hud = Config.Bind("Princess", "Hud", true, "Show the HUD with both princesses.");
            _labels = Config.Bind("Princess", "Labels", true, "Labels over cages and princesses you know about.");
            try { RegisterSettings(); } catch (Exception) { /* ModSettings isn't installed */ }
            _harmony = new Harmony("tabginstaller.princessmode");
            _harmony.PatchAll(typeof(PrincessModePlugin).Assembly);
            Logger.LogInfo("[Princess] Loaded; active on Capture the Princess servers.");
        }

        private void OnDestroy()
        {
            _harmony?.UnpatchSelf();
            ClearProps();
        }

        [MethodImpl(MethodImplOptions.NoInlining)]
        private void RegisterSettings()
        {
            ModSettings.ModSettingsUI.Register("Princess Mode", "Toggle Key", "Show or hide the HUD", _toggleKey);
            ModSettings.ModSettingsUI.Register("Princess Mode", "HUD", "Both princesses and the helicopter progress", _hud);
            ModSettings.ModSettingsUI.Register("Princess Mode", "Labels", "Labels over cages and princesses", _labels);
        }

        internal void Reset()
        {
            _state = null;
            _stateAt = -100f;
            _offer = null;
            _toasts.Clear();
            ClearProps();
        }

        internal void Handle(byte[] data)
        {
            if (!PrincessProtocol.TryRead(data, out PrincessState state, out Toast toast, out Offer offer))
                return;
            if (offer != null)
            {
                _offer = offer.Choices.Count > 0 ? offer : null;
                _offerUntil = Time.unscaledTime + offer.Seconds;
                return;
            }
            if (state != null)
            {
                if (_state == null)
                    Logger.LogInfo($"[Princess] Capture the Princess: you are {state.Mine.Name}, against {state.Theirs.Name}.");
                _state = state;
                _stateAt = Time.unscaledTime;
                return;
            }
            Logger.LogInfo("[Princess] " + toast.Text);
            _toasts.Add((toast, Time.unscaledTime + toast.Seconds));
            if (_toasts.Count > 4)
                _toasts.RemoveAt(0);
        }

        private void Update()
        {
            if (Input.GetKeyDown(_toggleKey.Value))
                _hud.Value = !_hud.Value;
            if (_offer != null)
            {
                if (Time.unscaledTime > _offerUntil)
                    _offer = null;
                else
                    for (int i = 0; i < _offer.Choices.Count && i < 3; i++)
                        if (Input.GetKeyDown(KeyCode.Alpha1 + i) || Input.GetKeyDown(KeyCode.Keypad1 + i))
                        {
                            UnityEngine.Object.FindObjectOfType<ServerConnector>()?.SendMessageToServer(
                                (EventCode)PrincessProtocol.EventCode, PrincessProtocol.CreatePick((byte)i), true);
                            _offer = null;
                            break;
                        }
            }
            if (Player.localPlayer != null && Time.unscaledTime >= _nextHello)
            {
                _nextHello = Time.unscaledTime + 3f;
                UnityEngine.Object.FindObjectOfType<ServerConnector>()?.SendMessageToServer(
                    (EventCode)PrincessProtocol.EventCode, PrincessProtocol.CreateHello(), true);
            }
            // Not on a Capture the Princess server (any more): nothing to show.
            if (_state != null && Time.unscaledTime - _stateAt > 6f)
                Reset();
            if (_state == null)
                return;
            string key = $"{_state.Bases[0].Cage}{_state.Bases[1].Cage}{_state.Bases[0].Turrets.Count}{_state.Bases[1].Turrets.Count}";
            if (key != _propsFor)
                BuildProps(key);
            // Turret heads turn: slowly on watch, fast while tracking a helicopter.
            int t = 0;
            foreach (Base b in _state.Bases)
                foreach (Turret turret in b.Turrets)
                {
                    if (t < _turretHeads.Count && _turretHeads[t] != null)
                        _turretHeads[t].Rotate(0f, (turret.Active ? 200f : 25f) * Time.deltaTime, 0f, Space.World);
                    t++;
                }
        }

        private void BuildProps(string key)
        {
            ClearProps();
            _propsFor = key;
            Logger.LogInfo($"[Princess] Cages at {_state.Bases[0].Cage} and {_state.Bases[1].Cage}, {_state.Bases[0].Turrets.Count + _state.Bases[1].Turrets.Count} turrets.");
            for (int team = 0; team < 2; team++)
            {
                Base b = _state.Bases[team];
                Color colour = TeamColours[team];
                var cage = new GameObject("PrincessCage " + b.Name);
                cage.transform.position = b.Cage;
                const int bars = 12;
                for (int i = 0; i < bars; i++)
                {
                    float a = i * Mathf.PI * 2f / bars;
                    Part(PrimitiveType.Cylinder, cage.transform, new Vector3(Mathf.Sin(a) * 1.8f, 1.6f, Mathf.Cos(a) * 1.8f), new Vector3(0.09f, 1.6f, 0.09f), colour);
                }
                Part(PrimitiveType.Cylinder, cage.transform, new Vector3(0f, 3.25f, 0f), new Vector3(3.9f, 0.06f, 3.9f), colour * 0.8f);
                Part(PrimitiveType.Cylinder, cage.transform, new Vector3(0f, 0.04f, 0f), new Vector3(3.9f, 0.04f, 3.9f), colour * 0.6f);
                _props.Add(cage);
                foreach (Turret turret in b.Turrets)
                {
                    var root = new GameObject("PrincessTurret " + b.Name);
                    root.transform.position = turret.Position;
                    Transform head = TurretModel(root.transform);
                    if (head == null)
                    {
                        // No model bundle in this build: a simple stand-in.
                        var grey = new Color(0.32f, 0.34f, 0.36f);
                        Part(PrimitiveType.Cylinder, root.transform, new Vector3(0f, 0.35f, 0f), new Vector3(1.4f, 0.35f, 1.4f), grey);
                        head = new GameObject("Head").transform;
                        head.SetParent(root.transform, false);
                        head.localPosition = new Vector3(0f, 1.0f, 0f);
                        Part(PrimitiveType.Cube, head, Vector3.zero, new Vector3(1.0f, 0.6f, 1.0f), grey * 1.2f);
                    }
                    _turretHeads.Add(head);
                    _props.Add(root);
                }
            }
        }

        private static GameObject _turretPrefab;
        private static bool _bundleTried;

        /// <summary>
        /// Kenney's missile turret (CC0, see THIRD-PARTY.md) from the embedded bundle, 2.5 times its
        /// size, without colliders, its materials remade with the game's standard shader. Returns
        /// the part that turns, or null without the bundle.
        /// </summary>
        private Transform TurretModel(Transform parent)
        {
            if (!_bundleTried)
            {
                _bundleTried = true;
                try
                {
                    using (var stream = typeof(PrincessModePlugin).Assembly.GetManifestResourceStream("princess.bundle"))
                    {
                        if (stream != null)
                        {
                            var bytes = new byte[stream.Length];
                            int read = 0;
                            while (read < bytes.Length)
                                read += stream.Read(bytes, read, bytes.Length - read);
                            var bundle = AssetBundle.LoadFromMemory(bytes);
                            _turretPrefab = bundle != null ? bundle.LoadAsset<GameObject>("weapon-turret") : null;
                        }
                    }
                }
                catch (Exception ex)
                {
                    Logger.LogWarning("[Princess] Turret model: " + ex.Message);
                }
                Logger.LogInfo("[Princess] Turret model " + (_turretPrefab != null ? "loaded" : "not available, using a stand-in"));
            }
            if (_turretPrefab == null)
                return null;
            var model = UnityEngine.Object.Instantiate(_turretPrefab, parent, false);
            model.transform.localScale = Vector3.one * 2.5f;
            foreach (var c in model.GetComponentsInChildren<Collider>(true))
                UnityEngine.Object.Destroy(c);
            // The bundle's shader may not be in the game; the game's own standard one is.
            var template = GameObject.CreatePrimitive(PrimitiveType.Cube);
            var standard = template.GetComponent<Renderer>().sharedMaterial;
            foreach (var r in model.GetComponentsInChildren<Renderer>(true))
            {
                var mats = r.sharedMaterials;
                for (int i = 0; i < mats.Length; i++)
                {
                    var m = new Material(standard);
                    if (mats[i] != null && mats[i].HasProperty("_MainTex"))
                        m.mainTexture = mats[i].mainTexture;
                    m.color = Color.white;
                    mats[i] = m;
                }
                r.sharedMaterials = mats;
            }
            UnityEngine.Object.Destroy(template);
            // Kenney's turret turns as a whole on its base; the whole model is the head.
            return model.transform;
        }

        private static void Part(PrimitiveType type, Transform parent, Vector3 at, Vector3 scale, Color colour, bool forward = false)
        {
            var go = GameObject.CreatePrimitive(type);
            // Decoration only: nothing collides with it, so it can't block shots or players.
            var collider = go.GetComponent<Collider>();
            if (collider != null)
                UnityEngine.Object.Destroy(collider);
            go.transform.SetParent(parent, false);
            go.transform.localPosition = at;
            go.transform.localScale = scale;
            if (forward)
                go.transform.localRotation = Quaternion.Euler(80f, 0f, 0f);
            var renderer = go.GetComponent<Renderer>();
            if (renderer != null)
                renderer.material.color = colour;
        }

        private void ClearProps()
        {
            foreach (GameObject go in _props)
                if (go != null)
                    UnityEngine.Object.Destroy(go);
            _props.Clear();
            _turretHeads.Clear();
            _propsFor = null;
        }

        private void OnGUI()
        {
            if (_state == null || Player.localPlayer == null)
                return;
            InitGui();
            if (_labels.Value)
                DrawLabels();
            if (_hud.Value)
                DrawHud();
            DrawOffer();
            DrawToasts();
        }

        private void DrawHud()
        {
            PrincessState s = _state;
            Base mine = s.Mine, theirs = s.Theirs;
            var lines = new List<(string text, Color colour)>
            {
                ($"CAPTURE THE PRINCESS · you are {mine.Name}", Color.white),
                ($"{mine.Princess}: {PrincessProtocol.Describe(s, mine, true)}", mine.Status == PrincessStatus.Taken ? new Color(1f, 0.45f, 0.4f) : TeamColours[s.Team & 1]),
                ($"{theirs.Princess}: {PrincessProtocol.Describe(s, theirs, false)}", theirs.Status == PrincessStatus.Taken ? new Color(0.5f, 1f, 0.55f) : TeamColours[(s.Team + 1) & 1]),
                (s.Needed > 0 ? $"Helicopter: {s.Kills} / {s.Needed} kills" : "", new Color(0.85f, 0.85f, 0.85f)),
            };
            if (s.Tokens > 0)
                lines.Add(("You can call a helicopter: throw your smoke grenade", new Color(0.5f, 1f, 0.55f)));
            float width = 430f, height = 10f + lines.Count * 21f;
            var box = new Rect(16f, 110f, width, height);
            Fill(box, new Color(0f, 0f, 0f, 0.45f));
            float y = box.y + 5f;
            for (int i = 0; i < lines.Count; i++)
            {
                if (lines[i].text.Length == 0)
                    continue;
                _line.normal.textColor = lines[i].colour;
                GUI.Label(new Rect(box.x + 10f, y, width - 20f, 21f), lines[i].text, i == 0 ? _title : _line);
                y += 21f;
            }
        }

        /// <summary>The loadout menu: three choices, a countdown, keys 1 to 3.</summary>
        private void DrawOffer()
        {
            if (_offer == null)
                return;
            float left = Mathf.Max(0f, _offerUntil - Time.unscaledTime);
            float width = 460f, row = 46f, height = 44f + _offer.Choices.Count * row;
            var box = new Rect((Screen.width - width) / 2f, Screen.height * 0.32f, width, height);
            Fill(box, new Color(0f, 0f, 0f, 0.6f));
            _title.normal.textColor = Color.white;
            GUI.Label(new Rect(box.x + 14f, box.y + 8f, width - 28f, 24f), $"CHOOSE YOUR LOADOUT · {left:0} s", _title);
            for (int i = 0; i < _offer.Choices.Count; i++)
            {
                var (name, items) = _offer.Choices[i];
                float y = box.y + 38f + i * row;
                Fill(new Rect(box.x + 10f, y, width - 20f, row - 6f), new Color(1f, 1f, 1f, 0.07f));
                _title.normal.textColor = TeamColours[_state != null ? _state.Team & 1 : 0];
                GUI.Label(new Rect(box.x + 18f, y + 2f, width - 36f, 22f), $"[{i + 1}]  {name}", _title);
                _line.normal.textColor = new Color(0.85f, 0.85f, 0.85f);
                GUI.Label(new Rect(box.x + 52f, y + 20f, width - 70f, 20f), items, _line);
            }
        }

        private void DrawToasts()
        {
            float now = Time.unscaledTime;
            _toasts.RemoveAll(t => now > t.until);
            float y = Screen.height * 0.22f;
            foreach (var (toast, until) in _toasts)
            {
                float alpha = Mathf.Clamp01((until - now) / 0.6f);
                Color colour = toast.Kind == PrincessProtocol.Good ? new Color(0.45f, 1f, 0.5f) : toast.Kind == PrincessProtocol.Bad ? new Color(1f, 0.45f, 0.4f) : Color.white;
                colour.a = alpha;
                float width = Mathf.Min(900f, Screen.width - 40f);
                var rect = new Rect((Screen.width - width) / 2f, y, width, 40f);
                Fill(rect, new Color(0f, 0f, 0f, 0.55f * alpha));
                _banner.normal.textColor = colour;
                GUI.Label(rect, toast.Text, _banner);
                y += 46f;
            }
        }

        private void DrawLabels()
        {
            Camera cam = Camera.main;
            if (cam == null)
                return;
            Vector3 me = Player.localPlayer.transform.position;
            for (int team = 0; team < 2; team++)
            {
                Base b = _state.Bases[team];
                Label(cam, b.Cage + Vector3.up * 3.8f, $"{b.Name} cage · {Vector3.Distance(me, b.Cage):0} m", TeamColours[team]);
                if (b.Known && b.Status != PrincessStatus.Dead)
                    Label(cam, b.At + Vector3.up * 2.4f, $"♛ {b.Princess} · {Vector3.Distance(me, b.At):0} m", TeamColours[team]);
            }
        }

        private void Label(Camera cam, Vector3 world, string text, Color colour)
        {
            Vector3 p = cam.WorldToScreenPoint(world);
            if (p.z <= 0f)
                return;
            var rect = new Rect(p.x - 150f, Screen.height - p.y - 22f, 300f, 22f);
            _label.normal.textColor = new Color(0f, 0f, 0f, 0.8f);
            GUI.Label(new Rect(rect.x + 1f, rect.y + 1f, rect.width, rect.height), text, _label);
            _label.normal.textColor = colour;
            GUI.Label(rect, text, _label);
        }

        private void Fill(Rect r, Color c)
        {
            Color old = GUI.color;
            GUI.color = c;
            GUI.DrawTexture(r, _white);
            GUI.color = old;
        }

        private void InitGui()
        {
            if (_white != null)
                return;
            _white = new Texture2D(1, 1);
            _white.SetPixel(0, 0, Color.white);
            _white.Apply();
            _title = new GUIStyle(GUI.skin.label) { fontSize = 14, fontStyle = FontStyle.Bold };
            _line = new GUIStyle(GUI.skin.label) { fontSize = 14 };
            _banner = new GUIStyle(GUI.skin.label) { fontSize = 22, fontStyle = FontStyle.Bold, alignment = TextAnchor.MiddleCenter };
            _label = new GUIStyle(GUI.skin.label) { fontSize = 13, fontStyle = FontStyle.Bold, alignment = TextAnchor.LowerCenter };
        }
    }

    [HarmonyPatch(typeof(ServerConnector), "SendInitRequest")]
    internal static class PrincessJoinPatch
    {
        private static void Prefix() => PrincessModePlugin.Instance?.Reset();
    }

    [HarmonyPatch(typeof(ServerConnector), "OnEvent")]
    internal static class PrincessReceivePatch
    {
        private static bool Prefix(ClientPackage clientPackage)
        {
            if ((byte)clientPackage.Code != PrincessProtocol.EventCode)
                return true;
            PrincessModePlugin.Instance?.Handle(clientPackage.Buffer);
            return false;
        }
    }
}
