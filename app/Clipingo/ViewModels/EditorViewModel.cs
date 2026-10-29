using System.Collections.ObjectModel;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One clip of the run in the editor's clip list, with its exports per format.</summary>
public partial class EditorClip(Candidate candidate) : ObservableObject
{
    public int Rank { get; } = candidate.Rank;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Info))]
    public partial Candidate Candidate { get; set; } = candidate;

    [ObservableProperty]
    public partial string Title { get; set; } = candidate.Title;

    /// <summary>Exported 9:16 / 16:9 .mp4, if any.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasVertical))]
    public partial string? Vertical { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasHorizontal))]
    public partial string? Horizontal { get; set; }

    /// <summary>First frame, for the card.</summary>
    [ObservableProperty]
    public partial string? Thumb { get; set; }

    public bool HasVertical => Vertical is not null;
    public bool HasHorizontal => Horizontal is not null;

    public string Info
    {
        get
        {
            var parts = Candidate.Pieces.Count();
            return EditorViewModel.Clock(Candidate.Length) + (parts > 1 ? $" · {parts} parts" : "") + (Candidate.Score > 0 ? $" · score {Candidate.Score}" : "");
        }
    }

    public string? Export(string format) => format == "vertical" ? Vertical : Horizontal;
}

/// <summary>One piece of the timeline: a source range (seconds), played in list order.</summary>
public partial class Segment(double start, double end) : ObservableObject
{
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Length), nameof(Label))]
    public partial double Start { get; set; } = start;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Length), nameof(Label))]
    public partial double End { get; set; } = end;

    /// <summary>Caption text of this piece (the words inside it; corrections are saved back into the transcript).</summary>
    [ObservableProperty]
    public partial string Text { get; set; } = "";

    /// <summary>1-based place on the timeline.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Label), nameof(TextName))]
    public partial int Number { get; set; }

    /// <summary>The transcript's text for this piece; <see cref="Text"/> differs once corrected.</summary>
    public string Original { get; set; } = "";

    public bool TextEdited => Text != Original;

    /// <summary>Called when the text is corrected (the editor saves it).</summary>
    public Action<Segment>? Edited { get; set; }

    partial void OnTextChanged(string value)
    {
        if (TextEdited)
            Edited?.Invoke(this);
    }

    public double Length => End - Start;
    public string Label => $"Part {Number} · {EditorViewModel.Time(Start)} – {EditorViewModel.Time(End)}";
    public string TextName => $"Caption text part {Number}";
}

/// <summary>A word on the timeline: source word plus where it plays in the edit.</summary>
public sealed record TimelineWord(string Text, double At, double Until);

/// <summary>What the player shows.</summary>
public enum PreviewMode { Edit, Source, Vertical, Horizontal }

/// <summary>
/// The clip editor. A clip is a list of <see cref="Segment"/>s cut from the run's video and played one after another:
/// split at the playhead, trim, delete, reorder, stitch in other clips or any marked part of the video. Title, caption
/// text and the export look are edited beside it; every change is saved by itself (with undo / redo), and Export
/// renders the clip (MP4 + SRT + Premiere XML) in the chosen format.
/// </summary>
public partial class EditorViewModel(CoreClient core, Func<CoreClient> connect) : ObservableObject
{
    public static readonly string[] Framings = ["Smart", "Full frame", "Classic"];
    private static readonly string[] FramingIds = ["smart", "full", "classic"];
    private static readonly string[] PositionIds = ["bottom", "middle", "top"];
    public static readonly string[] Speeds = ["1×", "1.1×", "1.25×", "1.5×", "1.75×", "2×"];
    private static readonly double[] SpeedValues = [1, 1.1, 1.25, 1.5, 1.75, 2];

    /// <summary>Length of a clip marked with New clip without marks.</summary>
    private const double NewClipSeconds = 30;

    /// <summary>Shortest piece a split or trim leaves.</summary>
    public const double MinSegment = 0.2;

    private RenderOptions _options = new("vertical", true, true, "pop", true, 1, "smart", false);
    private bool _loading;
    private readonly Stack<Snapshot> _undo = new();
    private readonly Stack<Snapshot> _redo = new();
    private CancellationTokenSource? _saveDelay;
    private CoreClient? _thumbCore;
    private readonly Dictionary<string, string> _thumbs = [];
    private List<Word> _words = [];

    private sealed record Snapshot((double Start, double End)[] Segments, string Title);

    public ObservableCollection<RunListing> Runs { get; } = [];
    public ObservableCollection<EditorClip> Clips { get; } = [];
    public ObservableCollection<CaptionPreset> CaptionStyles { get; } = [];
    public ObservableCollection<Segment> Segments { get; } = [];

    /// <summary>The timeline's words, on the edit's clock (before speed and dead-air cuts).</summary>
    public IReadOnlyList<TimelineWord> TimelineWords { get; private set; } = [];

    /// <summary>Raised when the timeline's pieces change (the page redraws it).</summary>
    public event Action? TimelineChanged;

    [ObservableProperty]
    public partial RunListing? SelectedRun { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasClip), nameof(NoClip), nameof(Exported))]
    public partial EditorClip? Selected { get; set; }

    [ObservableProperty]
    public partial Segment? SelectedSegment { get; set; }

    /// <summary>The run's video.</summary>
    [ObservableProperty]
    public partial string? MediaPath { get; set; }

    [ObservableProperty]
    public partial double Duration { get; private set; }

    /// <summary>Playhead on the edit's clock (seconds from the clip's first frame).</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(PositionText))]
    public partial double Position { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(LengthText), nameof(PositionText))]
    public partial double Length { get; private set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsEditPreview), nameof(IsSourcePreview), nameof(IsExportPreview))]
    public partial PreviewMode Preview { get; set; }

    /// <summary>Source mode: marked range of the video, to add to the timeline or make a new clip of.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(MarksText))]
    [NotifyCanExecuteChangedFor(nameof(AddMarkedCommand))]
    public partial double? MarkIn { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(MarksText))]
    [NotifyCanExecuteChangedFor(nameof(AddMarkedCommand))]
    public partial double? MarkOut { get; set; }

    /// <summary>Source mode playhead (source seconds).</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(PositionText))]
    public partial double SourcePosition { get; set; }

    [ObservableProperty]
    public partial string ClipTitle { get; set; } = "";

    /// <summary>Clip list filter: 0 all, 1 with a 9:16 export, 2 with a 16:9 export.</summary>
    [ObservableProperty]
    public partial int ClipFilter { get; set; }

    public ObservableCollection<EditorClip> VisibleClips { get; } = [];

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsVertical), nameof(ExportLabel), nameof(Exported))]
    public partial int FormatIndex { get; set; }

    [ObservableProperty]
    public partial int FramingIndex { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Style))]
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
    [NotifyCanExecuteChangedFor(nameof(ExportCommand))]
    public partial bool IsBusy { get; set; }

    /// <summary>0–100 while exporting.</summary>
    [ObservableProperty]
    public partial double ExportProgress { get; set; }

    public bool HasClip => Selected is not null;
    public bool NoClip => Selected is null;
    public bool IsVertical => FormatIndex == 0;
    public string Format => IsVertical ? "vertical" : "horizontal";
    public string ExportLabel => IsVertical ? "Export 9:16" : "Export 16:9";
    public bool IsEditPreview => Preview == PreviewMode.Edit;
    public bool IsSourcePreview => Preview == PreviewMode.Source;
    public bool IsExportPreview => Preview is PreviewMode.Vertical or PreviewMode.Horizontal;
    public CaptionPreset? Style => CaptionStyleIndex >= 0 && CaptionStyleIndex < CaptionStyles.Count ? CaptionStyles[CaptionStyleIndex] : null;

    /// <summary>This clip's export in the chosen format.</summary>
    public string? Exported => Selected?.Export(Format);

    public string LengthText => Clock(Length);

    public string PositionText => Preview switch
    {
        PreviewMode.Source => $"{Time(SourcePosition)} / {Time(Duration)}",
        PreviewMode.Edit => $"{Time(Position)} / {Time(Length)}",
        _ => "",
    };

    public string MarksText => MarkIn is null && MarkOut is null
        ? "Mark a part of the video with In (I) and Out (O)"
        : $"In {(MarkIn is { } i ? Time(i) : "–")}  ·  Out {(MarkOut is { } o ? Time(o) : "–")}" + (MarkIn is { } a && MarkOut is { } b && b > a ? $"  ·  {b - a:0.0}s" : "");

    /// <summary>1:05.3 / 1:02:07.0</summary>
    public static string Time(double s)
    {
        var t = TimeSpan.FromSeconds(Math.Max(0, s));
        return t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}.{t.Milliseconds / 100}" : $"{t.Minutes}:{t.Seconds:00}.{t.Milliseconds / 100}";
    }

    /// <summary>0:23 / 1:02:07</summary>
    public static string Clock(double s)
    {
        var t = TimeSpan.FromSeconds(Math.Max(0, Math.Round(s)));
        return t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}" : $"{t.Minutes}:{t.Seconds:00}";
    }

    // ── Runs and clips ──

    /// <summary>Fills the video picker; opens the newest run if nothing is open.</summary>
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
            SelectedRun = Runs.FirstOrDefault(r => r.Id == keep);
            _loading = false;
            if (openNewest && SelectedRun is null && Runs.Count > 0)
                SelectedRun = Runs[0];
        }
        catch (Exception ex) when (ex is CoreException or IOException)
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
        await FlushSaveAsync();
        try
        {
            var open = await core.CallAsync<RunOpen>("run.open", new { id });
            _words = await core.CallAsync<List<Word>>("run.words", new { id, start = 0, end = double.MaxValue });
            _thumbs.Clear();
            MediaPath = open.Run.Media;
            Duration = open.Run.MediaInfo.DurationS ?? 0;
            _options = open.Options;
            FormatIndex = _options.Format == "horizontal" ? 1 : 0;
            FramingIndex = Math.Max(0, Array.IndexOf(FramingIds, _options.Framing));
            CaptionStyleIndex = Math.Max(0, CaptionStyles.ToList().FindIndex(p => p.Id == _options.CaptionStyle));
            (BurnCaptions, ShowTitle, CutDeadAir, Fade) = (_options.Captions, _options.Title, _options.CutDeadAir, _options.Fade);
            CaptionPositionIndex = Math.Max(0, Array.IndexOf(PositionIds, _options.CaptionPosition));
            CaptionSize = Math.Round(Math.Clamp(_options.CaptionSize, 0.6, 1.6) * 100);
            SpeedIndex = Math.Max(0, Array.FindIndex(SpeedValues, v => Math.Abs(v - _options.Speed) < 0.01));
            Clips.Clear();
            foreach (var c in open.Run.Candidates)
                Clips.Add(WithExports(new EditorClip(c), open.Clips));
            FilterClips();
            Selected = Clips.FirstOrDefault();
            _ = LoadCardThumbsAsync();
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }

    private static EditorClip WithExports(EditorClip clip, IEnumerable<string> videos)
    {
        string? Of(string suffix) => videos.FirstOrDefault(v => Path.GetFileName(v) == $"clip_{clip.Rank:00}_{suffix}.mp4");
        (clip.Vertical, clip.Horizontal) = (Of("9x16"), Of("16x9"));
        return clip;
    }

    partial void OnClipFilterChanged(int value) => FilterClips();

    private void FilterClips()
    {
        VisibleClips.Clear();
        foreach (var c in Clips.Where(c => ClipFilter switch { 1 => c.HasVertical, 2 => c.HasHorizontal, _ => true }))
            VisibleClips.Add(c);
    }

    private async Task LoadCardThumbsAsync()
    {
        var clips = Clips.ToList();
        var paths = await ThumbsAsync(clips.Select(c => c.Candidate.Pieces.First().Start + 0.5).ToArray(), 90);
        for (var i = 0; i < clips.Count && i < paths.Count; i++)
            clips[i].Thumb = paths[i] is { Length: > 0 } p ? p : null;
    }

    /// <summary>Frames of the video at these source times (cached; "" when one cannot be read).</summary>
    public async Task<IReadOnlyList<string>> ThumbsAsync(double[] times, int height)
    {
        if (SelectedRun is not { } run || times.Length == 0)
            return [];
        string Key(double t) => $"{height}:{Math.Round(t, 1)}";
        var missing = times.Where(t => !_thumbs.ContainsKey(Key(t))).Distinct().ToArray();
        if (missing.Length > 0)
        {
            try
            {
                // Own connection: a filmstrip takes a moment and must not hold up saves.
                _thumbCore ??= connect();
                var got = await _thumbCore.CallAsync<List<string>>("run.thumbs", new { id = run.Id, times = missing, height });
                for (var i = 0; i < missing.Length && i < got.Count; i++)
                    _thumbs[Key(missing[i])] = got[i];
            }
            catch (Exception ex) when (ex is CoreException or IOException)
            {
                return times.Select(_ => "").ToList();
            }
        }
        return times.Select(t => _thumbs.GetValueOrDefault(Key(t), "")).ToList();
    }

    partial void OnSelectedChanged(EditorClip? oldValue, EditorClip? newValue) => _ = SwitchClipAsync(newValue);

    private async Task SwitchClipAsync(EditorClip? to)
    {
        await FlushSaveAsync();
        if (to is null)
        {
            Segments.Clear();
            Rebuild();
            return;
        }
        _loading = true;
        ClipTitle = to.Title;
        Segments.Clear();
        foreach (var (s, e) in to.Candidate.Pieces)
            Segments.Add(new Segment(s, e));
        _undo.Clear();
        _redo.Clear();
        NotifyHistory();
        _loading = false;
        Status = "";
        ErrorMessage = null;
        if (Preview != PreviewMode.Source)
            Preview = PreviewMode.Edit;
        Rebuild();
        SelectedSegment = Segments.FirstOrDefault();
        Position = 0;
    }

    // ── The timeline ──

    /// <summary>Renumbers the pieces, refreshes their text and the timeline words, and tells the page to redraw.</summary>
    private void Rebuild()
    {
        var words = new List<TimelineWord>();
        double at = 0;
        _loading = true;
        for (var i = 0; i < Segments.Count; i++)
        {
            var seg = Segments[i];
            seg.Number = i + 1;
            var inside = _words.Where(w => w.E > seg.Start && w.S < seg.End).ToList();
            if (!seg.TextEdited)
                (seg.Original, seg.Text) = (string.Join(" ", inside.Select(w => w.W)), string.Join(" ", inside.Select(w => w.W)));
            seg.Edited ??= _ => ScheduleSave();
            foreach (var w in inside)
                words.Add(new TimelineWord(w.W, at + Math.Max(0, w.S - seg.Start), at + Math.Min(seg.Length, w.E - seg.Start)));
            at += seg.Length;
        }
        _loading = false;
        TimelineWords = words;
        Length = at;
        if (Position > Length)
            Position = Length;
        TimelineChanged?.Invoke();
    }

    /// <summary>Where the edit's clock time <paramref name="t"/> plays: piece index and source time.</summary>
    public (int Index, double Source) Locate(double t)
    {
        double at = 0;
        for (var i = 0; i < Segments.Count; i++)
        {
            var s = Segments[i];
            if (t < at + s.Length || i == Segments.Count - 1)
                return (i, Math.Clamp(s.Start + t - at, s.Start, s.End));
            at += s.Length;
        }
        return (-1, 0);
    }

    /// <summary>The edit's clock time where piece <paramref name="index"/> starts.</summary>
    public double StartOf(int index) => Segments.Take(index).Sum(s => s.Length);

    /// <summary>Before a change: remember the timeline and title for undo.</summary>
    public void Checkpoint()
    {
        _undo.Push(Take());
        _redo.Clear();
        NotifyHistory();
    }

    /// <summary>A checkpoint that turned out not to change anything.</summary>
    public void DropCheckpoint()
    {
        if (_undo.Count > 0)
            _undo.Pop();
        NotifyHistory();
    }

    /// <summary>After a change: redraw and save soon.</summary>
    public void Changed()
    {
        Rebuild();
        ScheduleSave();
    }

    private Snapshot Take() => new(Segments.Select(s => (s.Start, s.End)).ToArray(), ClipTitle);

    private void Restore(Snapshot s)
    {
        var at = SelectedSegment is { } sel ? Segments.IndexOf(sel) : 0;
        _loading = true;
        Segments.Clear();
        foreach (var (a, b) in s.Segments)
            Segments.Add(new Segment(a, b));
        ClipTitle = s.Title;
        _loading = false;
        SelectedSegment = Segments.Count == 0 ? null : Segments[Math.Clamp(at, 0, Segments.Count - 1)];
        Changed();
    }

    private void NotifyHistory()
    {
        UndoCommand.NotifyCanExecuteChanged();
        RedoCommand.NotifyCanExecuteChanged();
    }

    private bool CanUndo() => _undo.Count > 0;

    private bool CanRedo() => _redo.Count > 0;

    [RelayCommand(CanExecute = nameof(CanUndo))]
    private void Undo()
    {
        _redo.Push(Take());
        Restore(_undo.Pop());
        NotifyHistory();
    }

    [RelayCommand(CanExecute = nameof(CanRedo))]
    private void Redo()
    {
        _undo.Push(Take());
        Restore(_redo.Pop());
        NotifyHistory();
    }

    /// <summary>Splits the piece under the playhead in two (S).</summary>
    [RelayCommand]
    private void Split()
    {
        var (i, src) = Locate(Position);
        if (i < 0)
            return;
        var seg = Segments[i];
        if (src - seg.Start < MinSegment || seg.End - src < MinSegment)
        {
            Status = "Move the playhead inside a part to split it";
            return;
        }
        Checkpoint();
        var right = new Segment(src, seg.End);
        seg.End = src;
        Segments.Insert(i + 1, right);
        SelectedSegment = right;
        Changed();
        Status = "Split";
    }

    /// <summary>Removes the selected piece; the rest close the gap (Delete).</summary>
    [RelayCommand]
    private void DeleteSegment()
    {
        if (SelectedSegment is not { } seg)
            return;
        if (Segments.Count == 1)
        {
            Status = "A clip needs at least one part";
            return;
        }
        Checkpoint();
        var at = Segments.IndexOf(seg);
        Segments.Remove(seg);
        SelectedSegment = Segments[Math.Min(at, Segments.Count - 1)];
        Changed();
        Seek(StartOf(Segments.IndexOf(SelectedSegment)));
    }

    /// <summary>Cuts the piece under the playhead from its start to the playhead (Q).</summary>
    [RelayCommand]
    private void TrimStart() => TrimAtPlayhead(start: true);

    /// <summary>Cuts the piece under the playhead from the playhead to its end (W).</summary>
    [RelayCommand]
    private void TrimEnd() => TrimAtPlayhead(start: false);

    private void TrimAtPlayhead(bool start)
    {
        var (i, src) = Locate(Position);
        if (i < 0)
            return;
        var seg = Segments[i];
        if ((start ? seg.End - src : src - seg.Start) < MinSegment)
            return;
        Checkpoint();
        if (start)
            seg.Start = src;
        else
            seg.End = src;
        SelectedSegment = seg;
        Changed();
        Seek(start ? StartOf(i) : StartOf(i) + seg.Length);
    }

    /// <summary>Live trim while an edge is dragged (the timeline calls <see cref="Checkpoint"/> first and <see cref="Changed"/> after).</summary>
    public void TrimTo(Segment seg, double start, double end)
    {
        seg.Start = Math.Clamp(start, 0, seg.End - MinSegment);
        seg.End = Math.Clamp(end, seg.Start + MinSegment, Duration > 0 ? Duration : double.MaxValue);
    }

    /// <summary>Moves a piece to another place on the timeline (dragged there).</summary>
    public void Move(Segment seg, int to)
    {
        var from = Segments.IndexOf(seg);
        to = Math.Clamp(to, 0, Segments.Count - 1);
        if (from < 0 || from == to)
            return;
        Checkpoint();
        Segments.Move(from, to);
        SelectedSegment = seg;
        Changed();
    }

    /// <summary>Stitches another clip's parts in after the selected part.</summary>
    [RelayCommand]
    private void InsertClip(EditorClip other) => Insert(other.Candidate.Pieces.ToArray(), $"Added “{other.Title}”");

    /// <summary>Source mode: adds the marked part of the video after the selected part.</summary>
    [RelayCommand(CanExecute = nameof(HasMarks))]
    private void AddMarked()
    {
        Insert([(MarkIn!.Value, MarkOut!.Value)], "Marked part added to the timeline");
        Preview = PreviewMode.Edit;
    }

    private bool HasMarks() => MarkIn is { } a && MarkOut is { } b && b - a >= MinSegment && HasClip;

    private void Insert((double Start, double End)[] pieces, string status)
    {
        if (Selected is null || pieces.Length == 0)
            return;
        Checkpoint();
        var at = SelectedSegment is { } s ? Segments.IndexOf(s) + 1 : Segments.Count;
        foreach (var (a, b) in pieces)
            Segments.Insert(at++, new Segment(a, b));
        SelectedSegment = Segments[at - 1];
        Changed();
        Seek(StartOf(at - 1));
        Status = status;
    }

    /// <summary>Moves the playhead on the edit's clock (the page seeks the player to it).</summary>
    public void Seek(double t) => Position = Math.Clamp(t, 0, Length);

    public void SeekSource(double t) => SourcePosition = Math.Clamp(t, 0, Duration > 0 ? Duration : double.MaxValue);

    [RelayCommand]
    private void SetMarkIn() => MarkIn = SourcePosition;

    [RelayCommand]
    private void SetMarkOut() => MarkOut = SourcePosition;

    /// <summary>A new clip: the marked part in Source mode, else 30 s from the playhead in the video.</summary>
    [RelayCommand]
    private async Task NewClipAsync()
    {
        if (SelectedRun is null)
            return;
        var from = IsSourcePreview ? SourcePosition : Locate(Position).Source;
        var (start, end) = MarkIn is { } a && MarkOut is { } b && b - a >= 1 ? (a, b) : (from, from + NewClipSeconds);
        end = Math.Min(end, Duration > 0 ? Duration : end);
        start = Math.Round(Math.Min(start, end - 1), 2);
        try
        {
            var c = await core.CallAsync<Candidate>("run.add_clip", new { id = SelectedRun.Id, start, end = Math.Round(end, 2), title = $"New clip at {Time(start)}" });
            var clip = new EditorClip(c);
            Clips.Add(clip);
            FilterClips();
            Selected = clip;
            (MarkIn, MarkOut) = (null, null);
            Preview = PreviewMode.Edit;
            Status = "New clip added";
            _ = LoadCardThumbsAsync();
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }

    /// <summary>Removes the selected clip from the run (exported files are kept). The page confirms first.</summary>
    public async Task DeleteClipAsync()
    {
        if (Selected is not { } clip || SelectedRun is null)
            return;
        _saveDelay?.Cancel();
        try
        {
            await core.CallAsync<object>("run.remove_clip", new { id = SelectedRun.Id, rank = clip.Rank });
            var at = Clips.IndexOf(clip);
            Clips.Remove(clip);
            FilterClips();
            _undo.Clear();
            Selected = Clips.Count == 0 ? null : Clips[Math.Min(at, Clips.Count - 1)];
            Status = "Clip removed";
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }

    // ── Saving ──

    partial void OnClipTitleChanged(string oldValue, string newValue)
    {
        if (_loading)
            return;
        // One undo step per typing burst (until the next save), not per key.
        if (!_titleTyping)
        {
            _undo.Push(new Snapshot(Segments.Select(s => (s.Start, s.End)).ToArray(), oldValue));
            _redo.Clear();
            NotifyHistory();
            _titleTyping = true;
        }
        ScheduleSave();
    }

    private bool _titleTyping;

    private void ScheduleSave()
    {
        if (_loading || Selected is null)
            return;
        Status = "Unsaved changes";
        _saveDelay?.Cancel();
        _saveDelay = new CancellationTokenSource();
        var token = _saveDelay.Token;
        _ = Task.Delay(700, token).ContinueWith(_ => SaveAsync(), token, TaskContinuationOptions.OnlyOnRanToCompletion, TaskScheduler.FromCurrentSynchronizationContext());
    }

    /// <summary>Saves a pending change now (before switching clip or video, and before export).</summary>
    private async Task FlushSaveAsync()
    {
        if (_saveDelay is { IsCancellationRequested: false } pending)
        {
            pending.Cancel();
            await SaveAsync();
        }
    }

    /// <summary>Saves the timeline, title and corrected captions into the run. True when saved.</summary>
    public async Task<bool> SaveAsync()
    {
        _saveDelay = null;
        _titleTyping = false;
        if (Selected is not { } clip || SelectedRun is not { } run || Segments.Count == 0)
            return false;
        if (string.IsNullOrWhiteSpace(ClipTitle))
        {
            Status = "A clip needs a title";
            return false;
        }
        try
        {
            var retexted = false;
            foreach (var seg in Segments.Where(s => s.TextEdited).ToList())
            {
                await core.CallAsync<List<Word>>("run.retext", new { id = run.Id, start = seg.Start, end = seg.End, text = seg.Text });
                seg.Original = seg.Text;
                retexted = true;
            }
            if (retexted)
                _words = await core.CallAsync<List<Word>>("run.words", new { id = run.Id, start = 0, end = double.MaxValue });
            var segments = Segments.Select(s => new[] { Math.Round(s.Start, 3), Math.Round(s.End, 3) }).ToArray();
            var saved = await core.CallAsync<Candidate>("run.edit", new { id = run.Id, rank = clip.Rank, title = ClipTitle.Trim(), segments });
            (clip.Candidate, clip.Title) = (saved, saved.Title);
            if (retexted)
                Rebuild();
            ErrorMessage = null;
            Status = "All changes saved";
            return true;
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
            Status = "";
            return false;
        }
    }

    // ── Export ──

    partial void OnFormatIndexChanged(int value) => OnPropertyChanged(nameof(Format));

    private bool CanExport() => !IsBusy;

    /// <summary>Saves, then renders this clip in the chosen format: MP4, SRT and a Premiere XML in the run's clips folder.</summary>
    [RelayCommand(CanExecute = nameof(CanExport))]
    private async Task ExportAsync()
    {
        if (Selected is not { } clip || SelectedRun is null)
            return;
        _saveDelay?.Cancel();
        if (!await SaveAsync())
            return;
        IsBusy = true;
        ErrorMessage = null;
        var options = _options with
        {
            Format = Format,
            Framing = FramingIds[Math.Clamp(FramingIndex, 0, 2)],
            CaptionStyle = Style?.Id ?? _options.CaptionStyle,
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
            var progress = new Progress<CoreProgress>(p =>
            {
                Status = p.Stage;
                ExportProgress = p.Total is 100 ? p.Done : 0;
            });
            var done = await session.CallAsync<List<RenderedClip>>("run.render", new { id = SelectedRun.Id, ranks = new[] { clip.Rank }, options }, default, progress);
            _options = options;
            var r = done[0];
            if (IsVertical)
                clip.Vertical = r.Video;
            else
                clip.Horizontal = r.Video;
            OnPropertyChanged(nameof(Exported));
            FilterClips();
            Status = $"Exported · {r.DurationS:0.0}s · {r.Encoder}";
            Preview = IsVertical ? PreviewMode.Vertical : PreviewMode.Horizontal;
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
            Status = "";
        }
        finally
        {
            IsBusy = false;
            ExportProgress = 0;
        }
    }

    [RelayCommand]
    private async Task RevealAsync()
    {
        var path = Exported ?? (SelectedRun is { } r ? r.ClipsDir : null);
        if (path is null || !await Shell.RevealAsync(path))
            ErrorMessage = "Nothing exported yet";
    }
}
