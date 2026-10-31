namespace Clipingo.Models;

// Mirrors of the Rust core's JSON (snake_case on the wire, see CoreJson).

public sealed partial record SourceInfo(
    string Kind,
    string Input,
    string Title,
    string? Channel,
    double? DurationS,
    long? Width,
    long? Height,
    double? Fps,
    string? Thumbnail,
    long? ViewCount,
    string? UploadDate,
    bool WasLive);

public sealed partial record ToolStatus(string Name, string Purpose, bool Required, bool Found, string? Version);

public sealed partial record EncoderStatus(string Name, string Label, bool Works, string? Error, string? Hint);

/// <summary>A clip. <c>Segments</c>: source ranges played one after another (cut and stitched in the editor); null = Start..End.</summary>
public sealed partial record Candidate(double Start, double End, int Score, int Rank, string Title, IReadOnlyList<string> Reasons, IReadOnlyList<string> Keywords, IReadOnlyList<double[]>? Segments = null)
{
    public IEnumerable<(double Start, double End)> Pieces => Segments is { Count: > 0 } s ? s.Select(p => (p[0], p[1])) : [(Start, End)];

    public double Length => Pieces.Sum(p => p.End - p.Start);
}

public sealed partial record TranscriptSummary(string Model, string? Language, int Words);

public sealed partial record RunResult(string Id, string Dir, SourceInfo Source, string Media, SourceInfo MediaInfo, TranscriptSummary Transcript, IReadOnlyList<Candidate> Candidates, string ClipsDir);

/// <summary>A transcribed word: text, start and end in source seconds.</summary>
public sealed partial record Word(string W, double S, double E);

/// <summary>run.render options (the core's RenderOptions).</summary>
public sealed partial record RenderOptions(string Format, bool CutDeadAir, bool Captions, string CaptionStyle, bool Title, double Speed, string Framing, bool VisionCheck, string CaptionPosition = "auto", double CaptionSize = 1, bool Fade = false, bool SmartZoom = true, bool SwapStack = false);

/// <summary>run.open: a run for the editor, the options of its last export and its exported videos.</summary>
public sealed partial record RunOpen(RunResult Run, RenderOptions Options, IReadOnlyList<string> Clips, string ClipsDir);

public sealed partial record RenderedClip(int Rank, string Title, string Video, string Srt, string PremiereXml, double DurationS, double RemovedS, string Encoder);

public sealed partial record ModelStatus(string Id, string Label, string Language, bool Installed, string? Path, int DownloadMb, bool NeedsConversion);

public sealed partial record SystemReport(
    string Os,
    IReadOnlyList<ToolStatus> Tools,
    IReadOnlyList<string> NvidiaGpus,
    IReadOnlyList<EncoderStatus> Encoders,
    string? SelectedEncoder,
    string? WhisperCli,
    IReadOnlyList<ModelStatus> SpeechModels);

public sealed partial record CaptionPreset(string Id, string Name, string Description, string Font, bool Italic, string Primary, string Highlight, bool Upper);

public sealed partial record RunListing(
    string Id,
    string Dir,
    SourceInfo Source,
    string Media,
    TranscriptSummary Transcript,
    IReadOnlyList<Candidate> Candidates,
    string ClipsDir,
    IReadOnlyList<string> Clips);

/// <summary>run.framing: one crop of the source scaled into part of the 1080×1920 frame. Keys are [t, left, top] (edit clock, source px).</summary>
public sealed partial record FramingPanel(int W, int H, IReadOnlyList<double[]> Keys, int DestY, int DestH);

/// <summary>run.framing: a punch-in: amount of extra zoom from T to End.</summary>
public sealed partial record FramingZoom(double T, double End, double Amount, bool Slow, string Word);

/// <summary>
/// run.framing: one camera shot of a clip and how it is framed in 9:16 (follow, split, stack, group, whole): why, how
/// many people, the layouts it can show, its zooms and the height of its captions in the 1920 px frame.
/// </summary>
public sealed partial record FramingShot(double Start, double End, string Layout, IReadOnlyList<FramingPanel> Panels, string Why, int People, IReadOnlyList<string> Options, IReadOnlyList<FramingZoom> Zooms, int CaptionY);
