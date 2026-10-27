using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>Create: choose a video, find clip moments, render the chosen ones.</summary>
public partial class CreateViewModel(CoreClient core, Func<CoreClient> connect) : ObservableObject
{
    public IReadOnlyList<string> Languages { get; } = ["Auto", "English", "Hinglish", "Hindi"];

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(ProbeCommand))]
    public partial string SourceInput { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasSource), nameof(Details), nameof(Channel))]
    [NotifyCanExecuteChangedFor(nameof(FindClipsCommand))]
    public partial SourceInfo? Source { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    public partial string? ErrorMessage { get; set; }

    [ObservableProperty]
    public partial string Language { get; set; } = "Auto";

    [ObservableProperty]
    public partial bool Vertical { get; set; } = true;

    [ObservableProperty]
    public partial bool CutDeadAir { get; set; } = true;

    [ObservableProperty]
    public partial bool BurnCaptions { get; set; } = true;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasRun), nameof(RunSummary))]
    [NotifyCanExecuteChangedFor(nameof(RenderCommand))]
    public partial RunResult? Run { get; set; }

    [ObservableProperty]
    public partial string StageText { get; set; } = "";

    /// <summary>0–100, or -1 when the current step has no measurable progress.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsIndeterminate))]
    public partial double ProgressValue { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasRendered))]
    public partial string? OutputFolder { get; set; }

    public ObservableCollection<CandidateItem> Candidates { get; } = [];
    public ObservableCollection<RenderedClip> Rendered { get; } = [];

    public bool HasSource => Source is not null;
    public bool HasError => !string.IsNullOrEmpty(ErrorMessage);
    public bool HasRun => Run is not null;
    public bool HasRendered => OutputFolder is not null;
    public bool IsIndeterminate => ProgressValue < 0;
    public string Channel => Source?.Channel ?? (Source?.Kind == "file" ? "Local file" : "");

    public string RunSummary => Run is null
        ? ""
        : $"{Run.Candidates.Count} moments · {Run.Transcript.Words:N0} words transcribed with the {Run.Transcript.Model} model";

    /// <summary>"1:02:33 · 1920×1080 · 30 fps · 12,345 views · 27 Oct 2026 · Was live".</summary>
    public string Details
    {
        get
        {
            if (Source is not { } s)
                return "";
            var parts = new List<string>();
            if (s.DurationS is { } d)
                parts.Add(TimeSpan.FromSeconds(Math.Round(d)).ToString(d >= 3600 ? @"h\:mm\:ss" : @"m\:ss"));
            if (s is { Width: { } w, Height: { } h })
                parts.Add($"{w}×{h}");
            if (s.Fps is { } fps)
                parts.Add($"{fps:0.##} fps");
            if (s.ViewCount is { } views)
                parts.Add($"{views:N0} views");
            if (s.UploadDate is { Length: 8 } date && DateTime.TryParseExact(date, "yyyyMMdd", null, System.Globalization.DateTimeStyles.None, out var when))
                parts.Add(when.ToString("d MMM yyyy"));
            if (s.WasLive)
                parts.Add("Was live");
            return string.Join(" · ", parts);
        }
    }

    private Progress<CoreProgress> TrackProgress() => new(p =>
    {
        StageText = p.Total is 100 ? $"{p.Stage} · {p.Done}%" : p.Total is > 0 ? $"{p.Stage} · {p.Done >> 20} / {p.Total.Value >> 20} MB" : p.Stage;
        ProgressValue = p.Total is > 0 ? p.Done * 100.0 / p.Total.Value : -1;
    });

    private bool CanProbe() => !string.IsNullOrWhiteSpace(SourceInput);

    [RelayCommand(CanExecute = nameof(CanProbe))]
    private async Task ProbeAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        Source = null;
        Run = null;
        Candidates.Clear();
        try
        {
            Source = await core.CallAsync<SourceInfo>("source.probe", new { input = SourceInput.Trim().Trim('"') }, ct);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }

    private bool CanFindClips() => Source is not null;

    [RelayCommand(CanExecute = nameof(CanFindClips))]
    private async Task FindClipsAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        Run = null;
        Candidates.Clear();
        Rendered.Clear();
        OutputFolder = null;
        // Own connection: transcribing a stream takes minutes and must not block other calls.
        await using var session = connect();
        try
        {
            var options = new { transcribe = new { language = Language.ToLowerInvariant() } };
            Run = await session.CallAsync<RunResult>("run.create", new { input = Source!.Input, options }, ct, TrackProgress());
            foreach (var c in Run.Candidates)
                Candidates.Add(new CandidateItem(c));
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
        finally
        {
            StageText = "";
        }
    }

    private bool CanRender() => Run is not null;

    [RelayCommand(CanExecute = nameof(CanRender))]
    private async Task RenderAsync(CancellationToken ct)
    {
        var ranks = Candidates.Where(c => c.IsSelected).Select(c => c.Rank).ToArray();
        if (ranks.Length == 0)
        {
            ErrorMessage = "Select at least one clip to render.";
            return;
        }
        ErrorMessage = null;
        Rendered.Clear();
        OutputFolder = null;
        await using var session = connect();
        try
        {
            var options = new { format = Vertical ? "vertical" : "horizontal", cut_dead_air = CutDeadAir, captions = BurnCaptions };
            var clips = await session.CallAsync<List<RenderedClip>>("run.render", new { id = Run!.Id, ranks, options }, ct, TrackProgress());
            foreach (var clip in clips)
                Rendered.Add(clip);
            OutputFolder = clips.Count > 0 ? Path.GetDirectoryName(clips[0].Video) : null;
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
        finally
        {
            StageText = "";
        }
    }

    [RelayCommand]
    private void OpenOutputFolder()
    {
        if (OutputFolder is { } dir && Directory.Exists(dir))
            Process.Start(new ProcessStartInfo("explorer.exe", $"\"{dir}\"") { UseShellExecute = false });
    }
}
