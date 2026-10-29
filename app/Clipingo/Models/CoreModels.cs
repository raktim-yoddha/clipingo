namespace Clipingo.Models;

// Mirrors of the Rust core's JSON (snake_case on the wire, see CoreClient.Json).

public sealed record SourceInfo(
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

public sealed record ToolStatus(string Name, string Purpose, bool Required, bool Found, string? Version);

public sealed record EncoderStatus(string Name, string Label, bool Works, string? Error, string? Hint);

/// <summary>A clip. <c>Segments</c>: source ranges played one after another (cut and stitched in the editor); null = Start..End.</summary>
public sealed record Candidate(double Start, double End, int Score, int Rank, string Title, IReadOnlyList<string> Reasons, IReadOnlyList<string> Keywords, IReadOnlyList<double[]>? Segments = null)
{
    public IEnumerable<(double Start, double End)> Pieces => Segments is { Count: > 0 } s ? s.Select(p => (p[0], p[1])) : [(Start, End)];

    public double Length => Pieces.Sum(p => p.End - p.Start);
}

public sealed record TranscriptSummary(string Model, string? Language, int Words);

public sealed record RunResult(string Id, string Dir, SourceInfo Source, string Media, SourceInfo MediaInfo, TranscriptSummary Transcript, IReadOnlyList<Candidate> Candidates, string ClipsDir);

/// <summary>A transcribed word: text, start and end in source seconds.</summary>
public sealed record Word(string W, double S, double E);

/// <summary>run.render options (the core's RenderOptions).</summary>
public sealed record RenderOptions(string Format, bool CutDeadAir, bool Captions, string CaptionStyle, bool Title, double Speed, string Framing, bool VisionCheck, string CaptionPosition = "bottom", double CaptionSize = 1, bool Fade = false);

/// <summary>run.open: a run for the editor, the options of its last export and its exported videos.</summary>
public sealed record RunOpen(RunResult Run, RenderOptions Options, IReadOnlyList<string> Clips, string ClipsDir);

public sealed record RenderedClip(int Rank, string Title, string Video, string Srt, string PremiereXml, double DurationS, double RemovedS, string Encoder);

public sealed record ModelStatus(string Id, string Label, string Language, bool Installed, string? Path, int DownloadMb, bool NeedsConversion);

public sealed record SystemReport(
    string Os,
    IReadOnlyList<ToolStatus> Tools,
    IReadOnlyList<string> NvidiaGpus,
    IReadOnlyList<EncoderStatus> Encoders,
    string? SelectedEncoder,
    string? WhisperCli,
    IReadOnlyList<ModelStatus> SpeechModels);

public sealed record CaptionPreset(string Id, string Name, string Description, string Font, bool Italic, string Primary, string Highlight, bool Upper);

public sealed record RunListing(
    string Id,
    string Dir,
    SourceInfo Source,
    string Media,
    TranscriptSummary Transcript,
    IReadOnlyList<Candidate> Candidates,
    string ClipsDir,
    IReadOnlyList<string> Clips);
