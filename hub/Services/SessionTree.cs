using System.Text.Json.Nodes;

namespace KlarkHub.Services;

/// <summary>The parts of a klark `session.snapshot` the UI shows.</summary>
public sealed record SessionTree(IReadOnlyList<WorkspaceInfo> Workspaces, IReadOnlyList<TabInfo> Tabs, IReadOnlyList<PaneInfo> Panes)
{
    public static readonly SessionTree Empty = new(Array.Empty<WorkspaceInfo>(), Array.Empty<TabInfo>(), Array.Empty<PaneInfo>());

    public static SessionTree Parse(JsonNode? snapshot)
    {
        if (snapshot is null) return Empty;
        var workspaces = Items(snapshot["workspaces"]).Select(w => new WorkspaceInfo(
            Str(w, "workspace_id"), Str(w, "label"), Int(w, "number"), Str(w, "agent_status"))).ToList();
        var tabs = Items(snapshot["tabs"]).Select(t => new TabInfo(
            Str(t, "tab_id"), Str(t, "workspace_id"), Str(t, "label"), Int(t, "number"))).ToList();
        var panes = Items(snapshot["panes"]).Select(p => new PaneInfo(
            Str(p, "pane_id"), Str(p, "workspace_id"), Str(p, "tab_id"),
            First(Str(p, "label"), Str(p, "title"), Str(p, "terminal_title_stripped"), Str(p, "pane_id")),
            First(Str(p, "display_agent"), Str(p, "agent")),
            Str(p, "agent_status"),
            First(Str(p, "foreground_cwd"), Str(p, "cwd")),
            p?["focused"]?.GetValue<bool>() ?? false)).ToList();
        return new SessionTree(workspaces, tabs, panes);
    }

    public string WorkspaceLabel(string id) =>
        Workspaces.FirstOrDefault(w => w.Id == id) is { } w ? First(w.Label, $"workspace {w.Number}") : id;

    private static IEnumerable<JsonNode?> Items(JsonNode? node) => node as JsonArray ?? new JsonArray();
    private static string Str(JsonNode? node, string key) =>
        node?[key] is JsonValue v && v.TryGetValue<string>(out var s) ? s : "";
    private static int Int(JsonNode? node, string key) =>
        node?[key] is JsonValue v && v.TryGetValue<int>(out var i) ? i : 0;
    private static string First(params string[] values) => values.FirstOrDefault(v => !string.IsNullOrWhiteSpace(v)) ?? "";
}

public sealed record WorkspaceInfo(string Id, string Label, int Number, string AgentStatus);
public sealed record TabInfo(string Id, string WorkspaceId, string Label, int Number);
public sealed record PaneInfo(string Id, string WorkspaceId, string TabId, string Title, string Agent, string AgentStatus, string Cwd, bool Focused)
{
    /// <summary>The pane's own name; else "agent · folder"; never a raw pane id when anything better exists.</summary>
    public string DisplayName
    {
        get
        {
            var title = Title.TrimStart('✳', '✻', '✽', '✶', '·', '*', '⠂', '⠐', ' ');
            var raw = title.Length == 0 || title == Id || System.Text.RegularExpressions.Regex.IsMatch(title, @"^w?\d+[:/]p?\d+$")
                      || title.Equals(Agent, StringComparison.OrdinalIgnoreCase) || title.Equals(Cwd) || title is "zsh" or "bash" or "sh" or "fish";
            if (!raw) return title;
            var folder = Folder;
            if (Agent.Length > 0) return folder.Length > 0 ? $"{Agent} · {folder}" : Agent;
            return folder.Length > 0 ? folder : title.Length > 0 ? title : Id;
        }
    }

    /// <summary>The last part of the working directory.</summary>
    public string Folder => Cwd.TrimEnd('/') is { Length: > 0 } c ? c[(c.LastIndexOf('/') + 1)..] : "";
}
