using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using UnityEngine;

namespace TabgInstaller.PrincessMode
{
    /// <summary>What the HUD knows about one princess.</summary>
    internal enum PrincessStatus : byte
    {
        /// At home or standing somewhere, nobody holding her.
        Free = 0,
        /// Her own team walks her (home, or somewhere in their territory).
        WithOwnTeam = 1,
        /// The other team holds her at gunpoint: she walks to their cage.
        Taken = 2,
        /// Both teams hold her: she stands still.
        Contested = 3,
        Dead = 4,
    }

    internal struct Turret
    {
        public Vector3 Position;
        public float Cooldown;
        public bool Active;
    }

    /// <summary>One team's base and its princess, as the server shows it to this team.</summary>
    internal sealed class Base
    {
        public string Name = "";
        public string Princess = "";
        public Vector3 Centre;
        public float Radius;
        public Vector3 Cage;
        public readonly List<Turret> Turrets = new List<Turret>();
        public byte PrincessIndex = 255;
        public PrincessStatus Status;
        /// <summary>Whether this team knows where she is (always for its own princess).</summary>
        public bool Known;
        public Vector3 At;
        public byte Stacks;
        public float Speed = 1f;
        /// <summary>Metres to the cage she's being walked to, or -1.</summary>
        public float ToGo = -1f;
    }

    internal sealed class PrincessState
    {
        public byte Team;
        public readonly Base[] Bases = { new Base(), new Base() };
        public int Kills;
        public int Needed;
        public int Tokens;

        public Base Mine => Bases[Team & 1];
        public Base Theirs => Bases[(Team + 1) & 1];
    }

    internal struct Toast
    {
        public string Text;
        public byte Kind;
        public float Seconds;
    }

    /// <summary>Loadouts to choose from after a (re)spawn; none means the choice is over.</summary>
    internal sealed class Offer
    {
        public readonly List<(string Name, string Items)> Choices = new List<(string, string)>();
        public float Seconds;
    }

    /// <summary>
    /// Event 239, <c>PRNC</c> v1, from the server's Capture the Princess mode: the client says hello
    /// every few seconds; the server sends its team's view of the match four times a second and
    /// short banners.
    /// </summary>
    internal static class PrincessProtocol
    {
        public const byte EventCode = 239;
        public const byte Version = 1;
        public const byte Hello = 1;
        public const byte State = 2;
        public const byte Toast = 3;
        public const byte OfferOp = 4;
        public const byte Pick = 5;
        public const byte Info = 0;
        public const byte Good = 1;
        public const byte Bad = 2;

        public static byte[] CreateHello()
        {
            return new byte[] { (byte)'P', (byte)'R', (byte)'N', (byte)'C', Version, Hello };
        }

        public static byte[] CreatePick(byte choice)
        {
            return new byte[] { (byte)'P', (byte)'R', (byte)'N', (byte)'C', Version, Pick, choice };
        }

        /// <summary>Reads one message: a state, a banner or a loadout offer.</summary>
        public static bool TryRead(byte[] data, out PrincessState state, out Toast toast)
        {
            return TryRead(data, out state, out toast, out _);
        }

        public static bool TryRead(byte[] data, out PrincessState state, out Toast toast, out Offer offer)
        {
            state = null;
            toast = default;
            offer = null;
            if (data == null || data.Length < 6 || data[0] != 'P' || data[1] != 'R' || data[2] != 'N' || data[3] != 'C' || data[4] != Version)
                return false;
            try
            {
                using (var r = new BinaryReader(new MemoryStream(data, 6, data.Length - 6), Encoding.UTF8))
                {
                    switch (data[5])
                    {
                        case State:
                            state = ReadState(r);
                            return r.BaseStream.Position == r.BaseStream.Length;
                        case OfferOp:
                            offer = new Offer();
                            int n = r.ReadByte();
                            if (n > 9)
                                return false;
                            for (int i = 0; i < n; i++)
                                offer.Choices.Add((Short(r.ReadString()), Short60(r.ReadString())));
                            offer.Seconds = Mathf.Clamp(Finite(r.ReadSingle()), 0f, 120f);
                            return r.BaseStream.Position == r.BaseStream.Length;
                        case Toast:
                            toast.Text = r.ReadString();
                            toast.Kind = r.ReadByte();
                            toast.Seconds = Mathf.Clamp(Finite(r.ReadSingle()), 1f, 15f);
                            return toast.Text.Length <= 300 && r.BaseStream.Position == r.BaseStream.Length;
                        default:
                            return false;
                    }
                }
            }
            catch (EndOfStreamException) { return false; }
            catch (IOException) { return false; }
            catch (ArgumentException) { return false; }
        }

        private static PrincessState ReadState(BinaryReader r)
        {
            var state = new PrincessState { Team = r.ReadByte() };
            foreach (Base b in state.Bases)
            {
                b.Name = Short(r.ReadString());
                b.Princess = Short(r.ReadString());
                b.Centre = Vector(r);
                b.Radius = Finite(r.ReadSingle());
                b.Cage = Vector(r);
                int turrets = r.ReadByte();
                for (int i = 0; i < turrets; i++)
                    b.Turrets.Add(new Turret { Position = Vector(r), Cooldown = Finite(r.ReadSingle()), Active = r.ReadByte() != 0 });
                b.PrincessIndex = r.ReadByte();
                b.Status = (PrincessStatus)Math.Min(r.ReadByte(), (byte)4);
                b.Known = r.ReadByte() != 0;
                if (b.Known)
                    b.At = Vector(r);
                b.Stacks = r.ReadByte();
                b.Speed = Finite(r.ReadSingle());
                b.ToGo = Finite(r.ReadSingle());
            }
            state.Kills = r.ReadUInt16();
            state.Needed = r.ReadUInt16();
            state.Tokens = r.ReadByte();
            return state;
        }

        private static string Short(string s) => s.Length > 40 ? s.Substring(0, 40) : s;

        private static string Short60(string s) => s.Length > 60 ? s.Substring(0, 60) : s;

        private static float Finite(float v) => float.IsNaN(v) || float.IsInfinity(v) ? 0f : v;

        private static Vector3 Vector(BinaryReader r)
        {
            var v = new Vector3(Finite(r.ReadSingle()), Finite(r.ReadSingle()), Finite(r.ReadSingle()));
            return v;
        }

        /// <summary>One line about a princess for the HUD.</summary>
        public static string Describe(PrincessState s, Base b, bool mine)
        {
            Base other = ReferenceEquals(b, s.Bases[0]) ? s.Bases[1] : s.Bases[0];
            string where = b.ToGo >= 0 ? $", {b.ToGo:0} m to the {other.Name} cage" : "";
            string faster = b.Stacks > 0 ? $" ({b.Speed * 100f:0} % speed)" : "";
            switch (b.Status)
            {
                case PrincessStatus.Dead:
                    return mine ? "down, back in a moment" : "down, back home in a moment";
                case PrincessStatus.Contested:
                    return "held by both teams, standing still";
                case PrincessStatus.Taken:
                    return mine ? $"TAKEN by {other.Name}!{where}{faster}" : $"walking with you{where}{faster}";
                case PrincessStatus.WithOwnTeam:
                    return mine ? "walking with your team" : (b.Known ? "her team is moving her" : "hidden");
                default:
                    return mine ? "safe" : (b.Known ? "spotted" : "hidden somewhere in " + b.Name);
            }
        }
    }
}
