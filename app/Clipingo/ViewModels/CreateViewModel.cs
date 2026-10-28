using System.Globalization;
using System.IO;
using Clipingo.Models;
using Clipingo.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Clipingo.ViewModels;

/// <summary>One line of the Review step; <c>Step</c> is where its Edit link goes.</summary>
public sealed record ReviewRow(string Label, string Value, string Step);

/// <summary>
/// The Create wizard (BridgeClip's): Video → Format → Clips → Captions → Review. Generate hands the job to
/// <see cref="JobsService"/>, so the next video can be queued right away.
/// </summary>
public partial class CreateViewModel : ObservableObject
{
    public static readonly string[] StepNames = ["Video", "Format", "Clips", "Captions", "Review"];

    private static readonly (string Title, string Description)[] StepText =
    [
        ("Choose a video", "A local file or a YouTube link. Optionally suggest where to find clips."),
        ("Format, framing and speed", "Choose the look and pace of every clip in this job."),
        ("What to clip, length and count", "Optionally describe the moments you want. Pick one or more lengths, or none for any length."),
        ("Captions", "Word-by-word captions burned into each clip. Every clip also gets an .srt for Premiere."),
        ("Review and generate", "Check the run, then generate. You can queue another video right after."),
    ];

    private static readonly (string Id, string Label, string Range, double Min, double Max)[] Lengths =
    [
        ("xshort", "Extra short", "10–30s", 10, 30),
        ("short", "Short", "30–60s", 30, 60),
        ("medium", "Medium", "1–2m", 60, 120),
        ("long", "Long", "2–5m", 120, 300),
        ("xlong", "Extra long", "5–10m", 300, 600),
        ("extended", "Extended", "10–15m", 600, 900),
        ("feature", "Feature", "15–30m", 900, 1800),
    ];

    private readonly CoreClient _core;
    private readonly JobsService _jobs;

    public CreateViewModel(CoreClient core, JobsService jobs)
    {
        _core = core;
        _jobs = jobs;
        Workflow = new OptionGroup(false,
            new OptionItem("automatic", "Automatic", "Beginner friendly", "Finds the moments, cuts them and exports your clips for you.") { Primary = "Find → Cut → Export" },
            new OptionItem("review", "Review & edit", "For advanced users", "Scores every moment; you choose which clips to export.") { Primary = "Find → You pick → Export" });
        AspectRatio = new OptionGroup(false,
            new OptionItem("vertical", "Vertical", "9:16", "Shorts, Reels, TikTok"),
            new OptionItem("horizontal", "Horizontal", "16:9", "YouTube, X, LinkedIn"));
        Framing = new OptionGroup(false,
            new OptionItem("smart", "Smart", "", "Auto frames each shot"),
            new OptionItem("full", "Full frame", "", "Follows the speaker"),
            new OptionItem("classic", "Classic", "", "Whole frame, blurred"));
        Speed = new OptionGroup(false,
            [.. new[] { 1.0, 1.1, 1.25, 1.5, 1.75, 2.0 }.Select(s => new OptionItem(
                s.ToString(CultureInfo.InvariantCulture), "", $"{s.ToString(CultureInfo.InvariantCulture)}×", s == 1 ? "Normal" : $"{Math.Round(60 / s)}s per minute"))]);
        Language = new OptionGroup(false,
            new OptionItem("auto", "Auto", "", "Detects English, else Hinglish"),
            new OptionItem("english", "English", "", "whisper large-v3-turbo"),
            new OptionItem("hinglish", "Hinglish", "", "Roman script · Hindi2Hinglish-Apex"),
            new OptionItem("hindi", "Hindi", "", "Devanagari · IISc Vaani large-v3"));
        ClipLength = new OptionGroup(true, [.. Lengths.Select(l => new OptionItem(l.Id, l.Label, l.Range))]);
        CaptionStyle = new OptionGroup(false);

        AspectRatio.SelectionChanged += () => OnPropertyChanged(nameof(IsVertical));
        Framing.SelectionChanged += () => OnPropertyChanged(nameof(IsSmartFraming));
        Speed.SelectionChanged += () => OnPropertyChanged(nameof(SpeedNote));
        ClipLength.SelectionChanged += () => OnPropertyChanged(nameof(LengthAside));
        Workflow.SelectionChanged += () => OnPropertyChanged(nameof(GenerateLabel));
        _ = LoadCaptionPresetsAsync();
    }

    public OptionGroup Workflow { get; }
    public OptionGroup AspectRatio { get; }
    public OptionGroup Framing { get; }
    public OptionGroup Speed { get; }
    public OptionGroup Language { get; }
    public OptionGroup ClipLength { get; }
    public OptionGroup CaptionStyle { get; }

    // ── Step ──────────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(StepTitle), nameof(StepDescription), nameof(HasBack), nameof(IsLastStep), nameof(NextLabel), nameof(ShowGenerateNow), nameof(ShowStepHeader))]
    [NotifyCanExecuteChangedFor(nameof(NextCommand), nameof(BackCommand))]
    public partial int Step { get; set; }

    public string StepTitle => StepText[Step].Title;
    public string StepDescription => StepText[Step].Description;
    public bool ShowStepHeader => Step > 0;
    public bool HasBack => Step > 0;
    public bool IsLastStep => Step == StepNames.Length - 1;
    public bool ShowGenerateNow => Step > 0 && !IsLastStep;
    public string NextLabel => IsLastStep ? "" : $"Next: {StepNames[Step + 1]}";
    public string GenerateLabel => Workflow.Selected == "review" ? "Find candidates" : "Generate clips";

    private bool CanNext() => Step < StepNames.Length - 1 && HasSource;

    [RelayCommand(CanExecute = nameof(CanNext))]
    private void Next() => Step++;

    private bool CanBack() => Step > 0;

    [RelayCommand(CanExecute = nameof(CanBack))]
    private void Back() => Step--;

    /// <summary>Stepper and Review "Edit" links. Steps after Video need a video first.</summary>
    [RelayCommand]
    private void GoTo(string step)
    {
        var i = int.Parse(step, CultureInfo.InvariantCulture);
        if (i == 0 || HasSource)
            Step = i;
    }

    // ── Video ─────────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(ProbeCommand))]
    public partial string SourceInput { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasSource), nameof(Details), nameof(Channel), nameof(FooterHint))]
    [NotifyCanExecuteChangedFor(nameof(NextCommand), nameof(GenerateCommand))]
    public partial SourceInfo? Source { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasError))]
    public partial string? ErrorMessage { get; set; }

    [ObservableProperty]
    public partial bool PreferRange { get; set; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(RangeError), nameof(HasRangeError))]
    [NotifyCanExecuteChangedFor(nameof(GenerateCommand))]
    public partial string RangeStart { get; set; } = "";

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(RangeError), nameof(HasRangeError))]
    [NotifyCanExecuteChangedFor(nameof(GenerateCommand))]
    public partial string RangeEnd { get; set; } = "";

    public bool HasSource => Source is not null;
    public bool HasError => !string.IsNullOrEmpty(ErrorMessage);
    public string Channel => Source?.Channel ?? (Source?.Kind == "file" ? "Local file" : "");
    public string FooterHint => !HasSource ? "Add a video to continue." : "Ctrl+Enter generates from any step";

    /// <summary>"1:02:33 · 1920×1080 · 30 fps · 12,345 views · 27 Oct 2026 · Was live".</summary>
    public string Details
    {
        get
        {
            if (Source is not { } s)
                return "";
            var parts = new List<string>();
            if (s.DurationS is { } d)
                parts.Add(Clock(d));
            if (s is { Width: { } w, Height: { } h })
                parts.Add($"{w}×{h}");
            if (s.Fps is { } fps)
                parts.Add($"{fps:0.##} fps");
            if (s.ViewCount is { } views)
                parts.Add($"{views:N0} views");
            if (s.UploadDate is { Length: 8 } date && DateTime.TryParseExact(date, "yyyyMMdd", null, DateTimeStyles.None, out var when))
                parts.Add(when.ToString("d MMM yyyy"));
            if (s.WasLive)
                parts.Add("Was live");
            return string.Join(" · ", parts);
        }
    }

    /// <summary>Seconds ("90") or a timecode ("1:30", "1:02:30"); empty = null.</summary>
    private static double? ParseTime(string text, out bool bad)
    {
        bad = false;
        if (string.IsNullOrWhiteSpace(text))
            return null;
        double total = 0;
        foreach (var part in text.Trim().Split(':'))
        {
            if (!double.TryParse(part, NumberStyles.Float, CultureInfo.InvariantCulture, out var v) || v < 0)
            {
                bad = true;
                return null;
            }
            total = total * 60 + v;
        }
        return total;
    }

    public string RangeError
    {
        get
        {
            var start = ParseTime(RangeStart, out var badStart);
            var end = ParseTime(RangeEnd, out var badEnd);
            if (badStart || badEnd)
                return "Use seconds (90) or a timecode (1:30).";
            if (end is { } e && e <= (start ?? 0))
                return "End must be after the start.";
            return "";
        }
    }

    public bool HasRangeError => PreferRange && RangeError.Length > 0;

    private bool CanProbe() => !string.IsNullOrWhiteSpace(SourceInput);

    [RelayCommand(CanExecute = nameof(CanProbe))]
    private async Task ProbeAsync(CancellationToken ct)
    {
        ErrorMessage = null;
        Source = null;
        try
        {
            Source = await _core.CallAsync<SourceInfo>("source.probe", new { input = SourceInput.Trim().Trim('"') }, ct);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = ex.Message;
        }
    }

    // ── Format ────────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    public partial bool CutDeadAir { get; set; } = true;

    /// <summary>Ask Antigravity CLI to look at shots Smart framing is unsure about.</summary>
    [ObservableProperty]
    public partial bool VisionCheck { get; set; }

    public bool IsVertical => AspectRatio.Selected == "vertical";
    public bool IsSmartFraming => Framing.Selected == "smart";
    private double SpeedValue => double.Parse(Speed.Selected, CultureInfo.InvariantCulture);

    public string SpeedNote => SpeedValue > 1
        ? $"Lengths refer to the original footage. At {Speed.Selected}×, 60 seconds becomes about {Math.Round(60 / SpeedValue)} seconds before dead-air cuts."
        : "";

    // ── Clips ─────────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    public partial string ClipRequest { get; set; } = "";

    [ObservableProperty]
    public partial bool AutoClipCount { get; set; } = true;

    [ObservableProperty]
    public partial double MaxClips { get; set; } = 5;

    public string LengthAside => ClipLength.SelectedItems.Count() is var n && n == 0 ? "Any length" : $"{n} selected";

    // ── Captions ──────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    public partial bool IncludeTitle { get; set; } = true;

    [ObservableProperty]
    public partial bool BurnCaptions { get; set; } = true;

    private async Task LoadCaptionPresetsAsync()
    {
        try
        {
            var presets = await _core.CallAsync<List<CaptionPreset>>("captions.presets", null);
            CaptionStyle.Replace(presets.Select(p => new OptionItem(p.Id, p.Name, "", p.Description)
            {
                Font = p.Font,
                Primary = p.Primary,
                Highlight = p.Highlight,
                Italic = p.Italic,
                Upper = p.Upper,
            }), "pop");
            OnPropertyChanged(nameof(ReviewRows));
        }
        catch (Exception ex) when (ex is CoreException or IOException)
        {
            ErrorMessage = $"Caption styles unavailable: {ex.Message}";
        }
    }

    // ── Review ────────────────────────────────────────────────────────────────────────────

    public IReadOnlyList<ReviewRow> ReviewRows
    {
        get
        {
            var lengths = ClipLength.SelectedItems.Select(i => i.Mono).ToList();
            var range = PreferRange && RangeError.Length == 0 && (RangeStart.Length > 0 || RangeEnd.Length > 0)
                ? $" · {(RangeStart.Length > 0 ? RangeStart : "start")} to {(RangeEnd.Length > 0 ? RangeEnd : "end")}"
                : "";
            var framing = IsVertical ? $"{Framing.SelectedItem?.Title} framing{(IsSmartFraming && VisionCheck ? " · AI vision" : "")}" : "Whole frame";
            return
            [
                new("Workflow", Workflow.Selected == "review" ? "Review & edit · pick clips before export" : "Automatic", "0"),
                new("Video", $"{Source?.Title ?? "—"}{range}", "0"),
                new("Format", $"{AspectRatio.SelectedItem?.Title} {AspectRatio.SelectedItem?.Mono} · {framing}", "1"),
                new("Pacing", CutDeadAir ? "Cut dead air" : "Keep pauses", "1"),
                new("Speed", $"{Speed.Selected}×{(SpeedValue == 1 ? " · Normal" : " · All exported clips")}", "1"),
                new("Language", $"{Language.SelectedItem?.Title} · {Language.SelectedItem?.Hint}", "2"),
                new("Clips", $"{(lengths.Count == 0 ? "Any length" : string.Join(", ", lengths))} · {(AutoClipCount ? "Clipingo decides how many" : $"Up to {MaxClips:0}")}", "2"),
                new("What to clip", string.IsNullOrWhiteSpace(ClipRequest) ? "The best moments" : ClipRequest.Trim(), "2"),
                new("Captions", BurnCaptions ? CaptionStyle.SelectedItem?.Title ?? "Pop" : "Off", "3"),
                new("Title", IncludeTitle ? "Shown at the top" : "Off", "3"),
            ];
        }
    }

    public string QueueNote => _jobs.ActiveCount >= JobsService.MaxParallel
        ? $"{_jobs.ActiveCount} jobs are running. This one waits in the queue and starts automatically."
        : _jobs.ActiveCount > 0
            ? $"Runs alongside {_jobs.ActiveCount} other job{(_jobs.ActiveCount == 1 ? "" : "s")}. Up to {JobsService.MaxParallel} run at once."
            : "Runs on this computer. Speech models and the face detector are local; nothing is uploaded.";

    partial void OnStepChanged(int value)
    {
        if (value == StepNames.Length - 1)
        {
            OnPropertyChanged(nameof(ReviewRows));
            OnPropertyChanged(nameof(QueueNote));
        }
    }

    // ── Generate ──────────────────────────────────────────────────────────────────────────

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsEditing))]
    public partial JobItem? StartedJob { get; set; }

    public bool IsEditing => StartedJob is null;

    private bool CanGenerate() => HasSource && !(PreferRange && RangeError.Length > 0);

    [RelayCommand(CanExecute = nameof(CanGenerate))]
    private void Generate()
    {
        var start = PreferRange ? ParseTime(RangeStart, out _) : null;
        var end = PreferRange ? ParseTime(RangeEnd, out _) : null;
        var lengths = ClipLength.SelectedItems.Select(i => Lengths.First(l => l.Id == i.Id)).Select(l => new[] { l.Min, l.Max }).ToArray();
        var find = new
        {
            lengths,
            count = AutoClipCount ? (int?)null : (int)MaxClips,
            within = start is null && end is null ? null : new[] { start ?? 0, end ?? 1e9 },
            request = string.IsNullOrWhiteSpace(ClipRequest) ? null : ClipRequest.Trim(),
        };
        var runOptions = new { transcribe = new { language = Language.Selected }, find };
        var renderOptions = new
        {
            format = AspectRatio.Selected,
            framing = Framing.Selected,
            vision_check = IsSmartFraming && VisionCheck,
            cut_dead_air = CutDeadAir,
            captions = BurnCaptions,
            caption_style = CaptionStyle.Selected is { Length: > 0 } s ? s : "pop",
            title = IncludeTitle,
            speed = SpeedValue,
        };
        var summary = string.Join(" · ", ReviewRows.Where(r => r.Label is "Format" or "Speed" or "Clips" or "Captions").Select(r => r.Value));
        StartedJob = _jobs.Start(new JobRequest(Source!.Input, Source.Title, Workflow.Selected != "review", runOptions, renderOptions, summary));
    }

    /// <summary>"Clip another video": a fresh wizard that keeps the chosen options.</summary>
    [RelayCommand]
    private void StartAnother()
    {
        StartedJob = null;
        Source = null;
        SourceInput = "";
        PreferRange = false;
        RangeStart = RangeEnd = ClipRequest = "";
        Step = 0;
    }

    private static string Clock(double s) => TimeSpan.FromSeconds(Math.Round(s)).ToString(s >= 3600 ? @"h\:mm\:ss" : @"m\:ss");
}
