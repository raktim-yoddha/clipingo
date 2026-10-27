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

public sealed record ModelStatus(string Id, string Label, string Language, bool Installed, string? Path, int DownloadMb, bool NeedsConversion);

public sealed record SystemReport(
    string Os,
    IReadOnlyList<ToolStatus> Tools,
    IReadOnlyList<string> NvidiaGpus,
    IReadOnlyList<EncoderStatus> Encoders,
    string? SelectedEncoder,
    string? WhisperCli,
    IReadOnlyList<ModelStatus> SpeechModels);
