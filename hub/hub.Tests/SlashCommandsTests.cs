using KlarkHub.Services;

namespace KlarkHub.Tests;

public class SlashCommandsTests
{
    [Theory]
    [InlineData("claude")]
    [InlineData("Claude Code")]
    [InlineData("devin")]
    [InlineData("Devin")]
    [InlineData("codex")]
    [InlineData("agy")]
    [InlineData("Antigravity")]
    [InlineData("gemini")]
    [InlineData("opencode")]
    [InlineData("cursor")]
    [InlineData("copilot")]
    [InlineData("grok")]
    public void KnownAgents_GetAList(string agent) => Assert.NotEmpty(SlashCommands.For(agent));

    [Theory]
    [InlineData("amp")]
    [InlineData("bash")]
    [InlineData("")]
    [InlineData(null)]
    public void OtherAgents_GetNone(string? agent) => Assert.Empty(SlashCommands.For(agent));

    [Theory]
    [InlineData("claude")]
    [InlineData("devin")]
    [InlineData("codex")]
    [InlineData("agy")]
    [InlineData("gemini")]
    [InlineData("opencode")]
    [InlineData("cursor")]
    [InlineData("copilot")]
    [InlineData("grok")]
    public void Lists_AreWellFormed(string agent)
    {
        var list = SlashCommands.For(agent);
        Assert.All(list, c =>
        {
            Assert.StartsWith("/", c.Name);
            Assert.DoesNotContain(' ', c.Name);
            Assert.False(string.IsNullOrWhiteSpace(c.Description));
        });
        Assert.Equal(list.Count, list.Select(c => c.Name).Distinct().Count());
    }

    [Fact]
    public void Claude_HasContextAndModel()
    {
        var names = SlashCommands.For("claude").Select(c => c.Name).ToList();
        Assert.Contains("/context", names);
        Assert.Contains("/model", names);
    }

    [Theory]
    [InlineData("codex", "/model", "/review", "/permissions")]
    [InlineData("agy", "/model", "/plan", "/codesearch")]
    [InlineData("antigravity", "/model", "/plan", "/codesearch")]
    [InlineData("gemini", "/memory", "/compress", "/init")]
    [InlineData("opencode", "/sessions", "/undo", "/connect")]
    [InlineData("cursor", "/model", "/summarize", "/rewind")]
    [InlineData("copilot", "/plan", "/login", "/delegate")]
    [InlineData("grok", "/compact", "/rewind", "/imagine")]
    public void VerifiedCommands_Present(string agent, string a, string b, string c)
    {
        var names = SlashCommands.For(agent).Select(x => x.Name).ToList();
        Assert.Contains(a, names);
        Assert.Contains(b, names);
        Assert.Contains(c, names);
    }

    [Fact]
    public void Names_DoNotCollide() =>
        Assert.NotSame(SlashCommands.For("codex"), SlashCommands.For("cursor"));
}
