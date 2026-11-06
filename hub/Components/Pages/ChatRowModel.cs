using KlarkHub.Services;

namespace KlarkHub.Components.Pages;

/// <summary>A row of the chat view. Stamp changes only when the row's content does.</summary>
public sealed record ChatRowModel(string Key, string Kind, IReadOnlyList<ChatItem> Items)
{
    public string Stamp { get; } = $"{Items.Count}:{Items[^1].Id}:{Items.Sum(i => i.Text?.Length ?? 0)}";

    /// <summary>Consecutive tool calls, results and thinking fold into one row.</summary>
    public static List<ChatRowModel> Build(IReadOnlyList<ChatItem> items)
    {
        var rows = new List<ChatRowModel>();
        List<ChatItem>? run = null;
        void Flush()
        {
            if (run is { Count: > 0 }) rows.Add(new ChatRowModel(run[0].Id, "tools", run));
            run = null;
        }
        foreach (var i in items)
        {
            if (i.Kind is "tool_use" or "tool_result" or "thinking")
            {
                (run ??= new List<ChatItem>()).Add(i);
                continue;
            }
            Flush();
            if (string.IsNullOrEmpty(i.Text) && i.Md is not { Count: > 0 }) continue;
            rows.Add(new ChatRowModel(i.Id, i.Role == "user" ? "user" : "text", new[] { i }));
        }
        Flush();
        return rows;
    }
}
