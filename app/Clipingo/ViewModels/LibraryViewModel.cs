using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One saved run in the Library.</summary>
public sealed class RunCard(RunListing run)
{
    public RunListing Run { get; } = run;
    public string Title => Run.Source.Title;

    public string Details
    {
        get
        {
            // Run ids start with the creation time (Unix seconds).
            var created = long.TryParse(Run.Id.Split('-')[0], out var secs)
                ? DateTimeOffset.FromUnixTimeSeconds(secs).ToLocalTime().ToString("d MMM yyyy, HH:mm")
                : Run.Id;
            return $"{created} · {Run.Candidates.Count} moments · {Run.Clips.Count} clips exported · {Run.Transcript.Words:N0} words ({Run.Transcript.Model})";
        }
    }

    public string Moments => string.Join("  ·  ", Run.Candidates.Take(4).Select(c => c.Title));

    public string Folder => Run.Clips.Count > 0 ? Path.GetDirectoryName(Run.Clips[0])! : Run.Dir;

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
    }
}
