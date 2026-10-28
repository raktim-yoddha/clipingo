using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One clip of the run in the editor's list.</summary>
public partial class EditorClip(Candidate candidate) : ObservableObject
{
    public int Rank { get; } = candidate.Rank;
    public int Score { get; } = candidate.Score;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(RangeText))]
    public partial double Start { get; set; } = candidate.Start;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(RangeText))]
    public partial double End { get; set; } = candidate.End;

    [ObservableProperty]
    public partial string Title { get; set; } = candidate.Title;

    /// <summary>The exported .mp4, if any.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(StateText))]
    public partial string? Video { get; set; }

    public string RangeText => $"{EditorViewModel.Time(Start)} – {EditorViewModel.Time(End)} · {End - Start:0}s";
    public string StateText => Video is null ? "Not exported" : "Exported";
}

/// <summary>
/// Clip editor: pick a clip of a run (or mark a new one), preview it on the source, trim it (with undo), fix its
/// title and caption text, choose the look (format, framing, caption style / position / size, speed, fades), then
/// export it again (MP4 + SRT + Premiere XML).
/// </summary>
public partial class EditorViewModel(CoreClient core, Func<CoreClient> connect) : ObservableObject
{
    public static readonly string[] Formats = ["Vertical 9:16", "Horizontal 16:9"];
    private static readonly string[] FormatIds = ["vertical", "horizontal"];
    public static readonly string[] Framings = ["Smart", "Full frame", "Classic"];
    private static readonly string[] FramingIds = ["smart", "full", "classic"];
    public static readonly string[] Positions = ["Bottom", "Middle", "Top"];
    private static readonly string[] PositionIds = ["bottom", "middle", "top"];
    public static readonly string[] Speeds = ["1×", "1.1×", "1.25×", "1.5×", "1.75×", "2×"];
    private static readonly double[] SpeedValues = [1, 1.1, 1.25, 1.5, 1.75, 2];

    /// <summary>Length of a clip marked with New clip.</summary>
    private const double NewClipSeconds = 30;

    /// <summary>Seconds of source shown around the clip on the timeline.</summary>
    private const double Margin = 15;

    private RenderOptions _options = new("vertical", true, true, "pop", true, 1, "smart", false);
    private (double Start, double End) _textRange;
    private bool _loading;
    private readonly Stack<(double Start, double End)> _undo = new();
    private readonly Stack<(double Start, double End)> _redo = new();

    /// <summary>The current trim's words (the page shows them as a clickable transcript).</summary>
    public IReadOnlyList<Word> Words { get; private set; } = [];

    public ObservableCollection<RunListing> Runs { get; } = [];
    public ObservableCollection<EditorClip> Clips { get; } = [];
    public ObservableCollection<CaptionPreset> CaptionStyles { get; } = [];

    [ObservableProperty]
    public partial RunListing? SelectedRun { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasClip), nameof(NoClip))]
    public partial EditorClip? Selected { get; set; }

    [ObservableProperty]
    public partial string? MediaPath { get; set; }

    public double Duration { get; private set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(StartText), nameof(LengthText), nameof(BeforeStars), nameof(InsideStars), nameof(AfterStars), nameof(WinStart))]
    public partial double Start { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(EndText), nameof(LengthText), nameof(BeforeStars), nameof(InsideStars), nameof(AfterStars), nameof(WinEnd))]
    public partial double End { get; set; }

    /// <summary>Playhead in source seconds (the page keeps it in sync with the player).</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(PositionText))]
    public partial double Position { get; set; }

    [ObservableProperty]
    public partial string ClipTitle { get; set; } = "";

    [ObservableProperty]
    public partial string CaptionText { get; set; } = "";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(SaveCommand), nameof(RevertCommand))]
    public partial bool IsDirty { get; set; }

    [ObservableProperty]
    public partial int FormatIndex { get; set; }

    [ObservableProperty]
    public partial int FramingIndex { get; set; }

    [ObservableProperty]
    public partial int CaptionStyleIndex { get; set; }

    [ObservableProperty]
    public partial bool BurnCaptions { get; set; } = true;

    [ObservableProperty]
    public partial bool ShowTitle { get; set; } = true;

    [ObservableProperty]
    public partial bool CutDeadAir { get; set; } = true;

    [ObservableProperty]
    public partial int CaptionPositionIndex { get; set; }

    /// <summary>Caption size, 60–160 % of the style's size.</summary>
    [ObservableProperty]
    public partial double CaptionSize { get; set; } = 100;

    [ObservableProperty]
    public partial int SpeedIndex { get; set; }

    [ObservableProperty]
    public partial bool Fade { get; set; }

    [ObservableProperty]
    public partial string Status { get; set; } = "";

    [ObservableProperty]
    public partial string? ErrorMessage { get; set; }

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(ExportCommand), nameof(SaveCommand))]
    public partial bool IsBusy { get; set; }

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(RevealVideoCommand), nameof(RevealXmlCommand))]
    public partial string? ExportedVideo { get; set; }

    public bool HasClip => Selected is not null;
    public bool NoClip => Selected is null;

    public double WinStart => Math.Max(0, Start - Margin);
    public double WinEnd => Math.Min(Duration > 0 ? Duration : End + Margin, End + Margin);
    public double BeforeStars => Math.Max(0.001, Start - WinStart);
    public double InsideStars => Math.Max(0.001, End - Start);
    public double AfterStars => Math.Max(0.001, WinEnd - End);
    public string StartText => Time(Start);
    public string EndText => Time(End);
    public string PositionText => Time(Position);
    public string LengthText => $"{End - Start:0.0}s";

    public string? ExportedXml => ExportedVideo is null ? null : Path.ChangeExtension(ExportedVideo, ".xml");

    /// <summary>1:05.3 / 1:02:07.0</summary>
    public static string Time(double s)
    {
        var t = TimeSpan.FromSeconds(Math.Max(0, s));
        return t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}.{t.Milliseconds / 100}" : $"{t.Minutes}:{t.Seconds:00}.{t.Milliseconds / 100}";
    }

    /// <summary>Fills the video picker; opens the newest run if nothing is open (unless a run is about to be).</summary>
    public async Task LoadRunsAsync(bool openNewest = true)
    {
        try
        {
            if (CaptionStyles.Count == 0)
                foreach (var p in await core.CallAsync<List<CaptionPreset>>("captions.presets", null))
                    CaptionStyles.Add(p);
            var runs = await core.CallAsync<List<RunListing>>("run.list", null);
            var keep = SelectedRun?.Id;
            _loading = true;
            Runs.Clear();
            foreach (var r in runs)
                Runs.Add(r);
            SelectedRun = Runs.FirstOrDefault(r => r.Id == keep) ?? SelectedRun;
            _loading = false;
            if (openNewest && SelectedRun is null && Runs.Count > 0)
                SelectedRun = Runs[0];
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
        }
        finally
        {
            _loading = false;
        }
    }

    /// <summary>Opens a run (and one of its clips) from Jobs or Library.</summary>
    public async Task OpenAsync(string runId, int? rank)
    {
        await LoadRunsAsync(openNewest: false);
        if (Runs.FirstOrDefault(r => r.Id == runId) is { } run)
        {
            // Opened here and awaited, so the clip below is picked from the loaded list.
            _loading = true;
            SelectedRun = run;
            _loading = false;
            await OpenRunAsync(run.Id);
        }
        if (rank is { } r && Clips.FirstOrDefault(c => c.Rank == r) is { } clip)
            Selected = clip;
    }

    partial void OnSelectedRunChanged(RunListing? value)
    {
        if (!_loading && value is not null)
            _ = OpenRunAsync(value.Id);
    }

    private async Task OpenRunAsync(string id)
    {
        ErrorMessage = null;
        try
        {
            var open = await core.CallAsync<RunOpen>("run.open", new { id });
            MediaPath = open.Run.Media;
            Duration = open.Run.MediaInfo.DurationS ?? 0;
            _options = open.Options;
            FormatIndex = Math.Max(0, Array.IndexOf(FormatIds, _options.Format));
            FramingIndex = Math.Max(0, Array.IndexOf(FramingIds, _options.Framing));
            CaptionStyleIndex = Math.Max(0, CaptionStyles.ToList().FindIndex(p => p.Id == _options.CaptionStyle));
            (BurnCaptions, ShowTitle, CutDeadAir, Fade) = (_options.Captions, _options.Title, _options.CutDeadAir, _options.Fade);
            CaptionPositionIndex = Math.Max(0, Array.IndexOf(PositionIds, _options.CaptionPosition));
            CaptionSize = Math.Round(Math.Clamp(_options.CaptionSize, 0.6, 1.6) * 100);
            SpeedIndex = Math.Max(0, Array.FindIndex(SpeedValues, v => Math.Abs(v - _options.Speed) < 0.01));
            Clips.Clear();
            foreach (var c in open.Run.Candidates)
                Clips.Add(new EditorClip(c) { Video = ExportOf(open.Clips, c.Rank) });
            Selected = Clips.FirstOrDefault();
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
        }
    }

    private static string? ExportOf(IEnumerable<string> videos, int rank) =>
        videos.FirstOrDefault(v => Path.GetFileName(v).StartsWith($"clip_{rank:00}_", StringComparison.Ordinal));

    partial void OnSelectedChanged(EditorClip? oldValue, EditorClip? newValue) => _ = SwitchClipAsync(oldValue, newValue);

    /// <summary>Unsaved edits of the clip being left are saved first, so switching clips never loses work.</summary>
    private async Task SwitchClipAsync(EditorClip? from, EditorClip? to)
    {
        if (from is not null && IsDirty && Clips.Contains(from))
            await SaveClipAsync(from);
        if (to is not null)
            await LoadClipAsync(to);
    }

    private async Task LoadClipAsync(EditorClip clip)
    {
        _loading = true;
        (Start, End, ClipTitle, Position) = (clip.Start, clip.End, clip.Title, clip.Start);
        ExportedVideo = clip.Video;
        CaptionText = "";
        Status = "";
        _undo.Clear();
        _redo.Clear();
        NotifyHistory();
        _loading = false;
        IsDirty = false;
        await LoadTextAsync();
    }

    /// <summary>The words of the current trim, as the caption text to correct.</summary>
    private async Task LoadTextAsync()
    {
        if (SelectedRun is null)
            return;
        try
        {
            var words = await core.CallAsync<List<Word>>("run.words", new { id = SelectedRun.Id, start = Start, end = End });
            _loading = true;
            CaptionText = string.Join(" ", words.Select(w => w.W));
            _textRange = (Start, End);
            Words = words;
            OnPropertyChanged(nameof(Words));
            _loading = false;
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
        }
        finally
        {
            _loading = false;
        }
    }

    private bool _textEdited;

    partial void OnCaptionTextChanged(string value) => Touch(text: true);
    partial void OnClipTitleChanged(string value) => Touch();
    partial void OnStartChanged(double value) => Touch();
    partial void OnEndChanged(double value) => Touch();

    private void Touch(bool text = false)
    {
        if (_loading)
            return;
        IsDirty = true;
        _textEdited |= text;
        Status = "";
    }

    private void SetRange(double start, double end, bool fromHistory = false)
    {
        start = Math.Clamp(start, 0, Duration > 0 ? Duration : double.MaxValue);
        end = Math.Clamp(end, 0, Duration > 0 ? Duration : double.MaxValue);
        if (end - start < 1)
        {
            ErrorMessage = "A clip must be at least 1 second long.";
            return;
        }
        ErrorMessage = null;
        if (!fromHistory)
        {
            _undo.Push((Start, End));
            _redo.Clear();
        }
        (Start, End) = (Math.Round(start, 2), Math.Round(end, 2));
        NotifyHistory();
    }

    private void NotifyHistory()
    {
        UndoTrimCommand.NotifyCanExecuteChanged();
        RedoTrimCommand.NotifyCanExecuteChanged();
    }

    private bool CanUndoTrim() => _undo.Count > 0;

    private bool CanRedoTrim() => _redo.Count > 0;

    [RelayCommand(CanExecute = nameof(CanUndoTrim))]
    private void UndoTrim()
    {
        _redo.Push((Start, End));
        var (s, e) = _undo.Pop();
        SetRange(s, e, fromHistory: true);
    }

    [RelayCommand(CanExecute = nameof(CanRedoTrim))]
    private void RedoTrim()
    {
        _undo.Push((Start, End));
        var (s, e) = _redo.Pop();
        SetRange(s, e, fromHistory: true);
    }

    /// <summary>Moves the playhead (shortcuts and the transcript); the page seeks the player to it.</summary>
    public void SeekTo(double seconds) => Position = Math.Clamp(seconds, 0, Duration > 0 ? Duration : double.MaxValue);

    /// <summary>Marks a new clip from the playhead (30 s, or to the end of the video).</summary>
    [RelayCommand]
    private async Task NewClipAsync()
    {
        if (SelectedRun is null)
            return;
        var start = Math.Round(Position, 2);
        var end = Math.Min(start + NewClipSeconds, Duration > 0 ? Duration : start + NewClipSeconds);
        try
        {
            var c = await core.CallAsync<Candidate>("run.add_clip", new { id = SelectedRun.Id, start, end, title = $"New clip at {Time(start)}" });
            var clip = new EditorClip(c);
            Clips.Add(clip);
            Selected = clip;
            Status = "New clip added. Trim it, name it, then export.";
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
        }
    }

    /// <summary>Removes the selected clip from the run (exported files are kept). The page confirms first.</summary>
    public async Task DeleteClipAsync()
    {
        if (Selected is not { } clip || SelectedRun is null)
            return;
        try
        {
            await core.CallAsync<object>("run.remove_clip", new { id = SelectedRun.Id, rank = clip.Rank });
            IsDirty = false;
            var at = Clips.IndexOf(clip);
            Clips.Remove(clip);
            Selected = Clips.Count == 0 ? null : Clips[Math.Min(at, Clips.Count - 1)];
            Status = "Clip removed";
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
        }
    }

    [RelayCommand]
    private void SetStart() => SetRange(Position, End);

    [RelayCommand]
    private void SetEnd() => SetRange(Start, Position);

    [RelayCommand]
    private void NudgeStart(string seconds) => SetRange(Start + double.Parse(seconds, System.Globalization.CultureInfo.InvariantCulture), End);

    [RelayCommand]
    private void NudgeEnd(string seconds) => SetRange(Start, End + double.Parse(seconds, System.Globalization.CultureInfo.InvariantCulture));

    private bool CanSave() => IsDirty && !IsBusy;

    /// <summary>Saves trim, title and corrected captions into the run. True when saved (or nothing to save).</summary>
    [RelayCommand(CanExecute = nameof(CanSave))]
    private Task<bool> SaveAsync() => Selected is { } clip ? SaveClipAsync(clip) : Task.FromResult(false);

    private async Task<bool> SaveClipAsync(EditorClip clip)
    {
        if (SelectedRun is null)
            return false;
        ErrorMessage = null;
        try
        {
            if (_textEdited)
            {
                await core.CallAsync<List<Word>>("run.retext", new { id = SelectedRun.Id, start = _textRange.Start, end = _textRange.End, text = CaptionText });
                _textEdited = false;
            }
            var saved = await core.CallAsync<Candidate>("run.edit", new { id = SelectedRun.Id, rank = clip.Rank, start = Start, end = End, title = ClipTitle });
            (clip.Start, clip.End, clip.Title) = (saved.Start, saved.End, saved.Title);
            IsDirty = false;
            Status = "Saved";
            if (clip == Selected && _textRange != (Start, End))
                await LoadTextAsync();
            return true;
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
            return false;
        }
    }

    [RelayCommand]
    private Task RevertAsync() => Selected is { } clip ? LoadClipAsync(clip) : Task.CompletedTask;

    private bool CanExport() => !IsBusy;

    /// <summary>Saves, then renders this clip again: MP4, SRT and a Premiere XML in the run's clips folder.</summary>
    [RelayCommand(CanExecute = nameof(CanExport))]
    private async Task ExportAsync()
    {
        if (Selected is not { } clip || SelectedRun is null)
            return;
        if (IsDirty && !await SaveAsync())
            return;
        IsBusy = true;
        ErrorMessage = null;
        var options = _options with
        {
            Format = FormatIds[Math.Clamp(FormatIndex, 0, 1)],
            Framing = FramingIds[Math.Clamp(FramingIndex, 0, 2)],
            CaptionStyle = CaptionStyleIndex >= 0 && CaptionStyleIndex < CaptionStyles.Count ? CaptionStyles[CaptionStyleIndex].Id : _options.CaptionStyle,
            Captions = BurnCaptions,
            Title = ShowTitle,
            CutDeadAir = CutDeadAir,
            CaptionPosition = PositionIds[Math.Clamp(CaptionPositionIndex, 0, 2)],
            CaptionSize = Math.Clamp(CaptionSize, 60, 160) / 100,
            Speed = SpeedValues[Math.Clamp(SpeedIndex, 0, SpeedValues.Length - 1)],
            Fade = Fade,
        };
        try
        {
            // Own connection: an export takes a while and must not hold up the editor's other calls.
            await using var session = connect();
            var progress = new Progress<CoreProgress>(p => Status = p.Total is 100 ? $"{p.Stage} · {p.Done}%" : p.Stage);
            var done = await session.CallAsync<List<RenderedClip>>("run.render", new { id = SelectedRun.Id, ranks = new[] { clip.Rank }, options }, default, progress);
            _options = options;
            var r = done[0];
            clip.Video = r.Video;
            ExportedVideo = r.Video;
            Status = $"Exported · {r.DurationS:0.0}s · {r.Encoder}";
        }
        catch (Exception ex)
        {
            ErrorMessage = ex.Message;
            Status = "";
        }
        finally
        {
            IsBusy = false;
        }
    }

    private bool HasExport() => ExportedVideo is not null && File.Exists(ExportedVideo);

    [RelayCommand(CanExecute = nameof(HasExport))]
    private async Task RevealVideoAsync()
    {
        if (!await Shell.RevealAsync(ExportedVideo!))
            ErrorMessage = $"Could not open {Path.GetDirectoryName(ExportedVideo)}";
    }

    [RelayCommand(CanExecute = nameof(HasExport))]
    private async Task RevealXmlAsync()
    {
        if (!await Shell.RevealAsync(ExportedXml!))
            ErrorMessage = $"Could not open {Path.GetDirectoryName(ExportedXml)}";
    }
}
