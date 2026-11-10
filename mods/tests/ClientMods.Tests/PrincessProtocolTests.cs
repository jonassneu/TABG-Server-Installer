using System.IO;
using System.Text;
using FluentAssertions;
using TabgInstaller.PrincessMode;
using UnityEngine;
using Xunit;

namespace ClientMods.Tests
{
    public class PrincessProtocolTests
    {
        private static BinaryWriter Message(byte op, MemoryStream stream)
        {
            var w = new BinaryWriter(stream, Encoding.UTF8);
            w.Write(Encoding.ASCII.GetBytes("PRNC"));
            w.Write((byte)1);
            w.Write(op);
            return w;
        }

        private static void Vector(BinaryWriter w, float x, float y, float z)
        {
            w.Write(x);
            w.Write(y);
            w.Write(z);
        }

        /// <summary>The layout the server writes (room/princess.rs, princess_sync).</summary>
        private static byte[] State()
        {
            var stream = new MemoryStream();
            var w = Message(2, stream);
            w.Write((byte)1); // we are team 1 (City)
            // Castle: one turret, its princess taken by us and known.
            w.Write("Castle");
            w.Write("Blue Princess");
            Vector(w, -698, 135, 545);
            w.Write(95f);
            Vector(w, -724, 135, 553);
            w.Write((byte)1);
            Vector(w, -678, 145, 497);
            w.Write(3.5f);
            w.Write((byte)1);
            w.Write((byte)253);
            w.Write((byte)2);
            w.Write((byte)1);
            Vector(w, -650, 120, 300);
            w.Write((byte)1);
            w.Write(1.15f);
            w.Write(320f);
            // City: no turrets, our princess safe at home.
            w.Write("City");
            w.Write("Pink Princess");
            Vector(w, -647, 111, -11);
            w.Write(120f);
            Vector(w, -610, 111, 37);
            w.Write((byte)0);
            w.Write((byte)252);
            w.Write((byte)0);
            w.Write((byte)1);
            Vector(w, -636, 112, -46);
            w.Write((byte)0);
            w.Write(1f);
            w.Write(-1f);
            w.Write((ushort)2);
            w.Write((ushort)3);
            w.Write((byte)1);
            return stream.ToArray();
        }

        [Fact]
        public void State_is_read_from_the_servers_layout()
        {
            PrincessProtocol.TryRead(State(), out PrincessState s, out _).Should().BeTrue();
            s.Team.Should().Be(1);
            s.Mine.Name.Should().Be("City");
            s.Theirs.Princess.Should().Be("Blue Princess");
            s.Theirs.Status.Should().Be(PrincessStatus.Taken);
            s.Theirs.Turrets.Should().HaveCount(1);
            s.Theirs.Turrets[0].Active.Should().BeTrue();
            s.Theirs.At.Should().Be(new Vector3(-650, 120, 300));
            s.Theirs.ToGo.Should().Be(320f);
            (s.Kills, s.Needed, s.Tokens).Should().Be((2, 3, 1));
            PrincessProtocol.Describe(s, s.Theirs, false).Should().Contain("walking with you").And.Contain("320 m to the City cage");
            PrincessProtocol.Describe(s, s.Mine, true).Should().Be("safe");
        }

        [Fact]
        public void Loadout_offers_and_picks()
        {
            var stream = new MemoryStream();
            var w = Message(4, stream);
            w.Write((byte)2);
            w.Write("Sniper");
            w.Write("AWP, Glockinator, 3 bandages");
            w.Write("Rusher");
            w.Write("Vector, AA-12, 3 bandages");
            w.Write(30f);
            PrincessProtocol.TryRead(stream.ToArray(), out _, out _, out Offer offer).Should().BeTrue();
            offer.Choices.Should().HaveCount(2);
            offer.Choices[1].Name.Should().Be("Rusher");
            offer.Seconds.Should().Be(30f);
            PrincessProtocol.CreatePick(2).Should().Equal((byte)'P', (byte)'R', (byte)'N', (byte)'C', (byte)1, (byte)5, (byte)2);
        }

        [Fact]
        public void Banners_and_broken_messages()
        {
            var stream = new MemoryStream();
            var w = Message(3, stream);
            w.Write("You can now spawn a helicopter!");
            w.Write((byte)1);
            w.Write(4f);
            PrincessProtocol.TryRead(stream.ToArray(), out PrincessState state, out Toast toast).Should().BeTrue();
            state.Should().BeNull();
            toast.Text.Should().Be("You can now spawn a helicopter!");
            toast.Kind.Should().Be(PrincessProtocol.Good);

            byte[] cut = State();
            System.Array.Resize(ref cut, cut.Length - 3);
            PrincessProtocol.TryRead(cut, out _, out _).Should().BeFalse();
            PrincessProtocol.TryRead(new byte[] { 1, 2, 3 }, out _, out _).Should().BeFalse();
            PrincessProtocol.CreateHello().Should().Equal((byte)'P', (byte)'R', (byte)'N', (byte)'C', (byte)1, (byte)1);
        }
    }
}
