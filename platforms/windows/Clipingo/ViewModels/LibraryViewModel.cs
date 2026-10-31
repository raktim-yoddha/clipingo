using System.Text.Json;
using System.Text.Json.Nodes;
using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One saved run in the Library.</summary>
public partial class RunCard(RunListing run) : ObservableObject
{
    public RunListing Run { get; } = run;
    public string Title => Run.Source.Title;

    /// <summary>The YouTube thumbnail, or a frame of the video (filled in by the Library).</summary>
    [ObservableProperty]
    public partial string? Thumb { get; set; } = run.Source.Thumbnail;

    public string Details
    {
        get
        {
            // Run ids start with the creation time (Unix seconds).
            var created = long.TryParse(Run.Id.Split('-')[0], out var secs)
                ? DateTimeOffset.FromUnixTimeSeconds(secs).ToLocalTime().ToString("d MMM yyyy, HH:mm")
                : Run.Id;
            var vertical = Run.Clips.Count(c => c.EndsWith("_9x16.mp4", StringComparison.Ordinal));
            var exported = Run.Clips.Count == 0 ? "nothing exported yet"
                : string.Join(" + ", new[] { (vertical, "9:16"), (Run.Clips.Count - vertical, "16:9") }.Where(p => p.Item1 > 0).Select(p => $"{p.Item1} × {p.Item2}")) + " exported";
            return $"{created} · {Run.Candidates.Count} clip{(Run.Candidates.Count == 1 ? "" : "s")} · {exported}";
        }
    }

    public string Folder => Run.Clips.Count > 0 ? Path.GetDirectoryName(Run.Clips[0])! : Directory.Exists(Run.ClipsDir) ? Run.ClipsDir : Run.Dir;

    public async void OpenFolder() => await Shell.RevealAsync(Folder);
}

/// <summary>Library: every saved run, newest first, from clipd's run.list.</summary>
public partial class LibraryViewModel(CoreClient core) : ObservableObject
{
    public ObservableCollection<RunCard> Runs { get; } = [];

    [ObservableProperty]
    public partial string? ErrorMessage { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsEmpty))]
    public partial bool Loaded { get; set; }

    public bool IsEmpty => Loaded && Runs.Count == 0;

    [RelayCommand]
    private async Task RefreshAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        try
        {
            var runs = await core.CallAsync<List<RunListing>>("run.list", null, ct);
            Runs.Clear();
            foreach (var r in runs)
                Runs.Add(new RunCard(r));
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
        Loaded = true;
        OnPropertyChanged(nameof(IsEmpty));
        // Local videos: a frame from the first clip.
        foreach (var card in Runs.Where(c => string.IsNullOrEmpty(c.Thumb)).ToList())
        {
            try
            {
                var at = card.Run.Candidates.FirstOrDefault()?.Start ?? 1;
                var paths = await core.CallAsync<List<string>>("run.thumbs", new JsonObject { ["id"] = card.Run.Id, ["times"] = CoreJson.Node(new[] { at + 0.5 }), ["height"] = 120 }, ct);
                card.Thumb = paths.FirstOrDefault(p => p.Length > 0);
            }
            catch (Exception ex) when (ex is CoreException or IOException)
            {
            }
        }
    }

    /// <summary>Removes a run from the Library (the page confirms first). Exported clips stay on disk.</summary>
    public async Task DeleteAsync(RunCard card)
    {
        try
        {
            await core.CallAsync<JsonElement>("run.delete", new JsonObject { ["id"] = card.Run.Id });
            Runs.Remove(card);
            OnPropertyChanged(nameof(IsEmpty));
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }
}
