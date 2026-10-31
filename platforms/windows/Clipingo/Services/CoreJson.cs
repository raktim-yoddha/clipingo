using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using System.Text.Json.Serialization.Metadata;
using Clipingo.Models;

namespace Clipingo.Services;

/// <summary>
/// Everything that crosses the pipe, generated at build time: the app is compiled ahead of time and has no reflection
/// to map JSON onto records. A reply type missing here fails at the first call, with its name.
/// </summary>
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower)]
[JsonSerializable(typeof(CoreProgress))]
[JsonSerializable(typeof(JsonElement))]
[JsonSerializable(typeof(Candidate))]
[JsonSerializable(typeof(RunOpen))]
[JsonSerializable(typeof(RunResult))]
[JsonSerializable(typeof(RenderOptions))]
[JsonSerializable(typeof(SourceInfo))]
[JsonSerializable(typeof(SystemReport))]
[JsonSerializable(typeof(List<CaptionPreset>))]
[JsonSerializable(typeof(List<FramingShot>))]
[JsonSerializable(typeof(List<RenderedClip>))]
[JsonSerializable(typeof(List<RunListing>))]
[JsonSerializable(typeof(List<Word>))]
[JsonSerializable(typeof(List<string>))]
[JsonSerializable(typeof(double[]))]
[JsonSerializable(typeof(double[][]))]
[JsonSerializable(typeof(int[]))]
internal sealed partial class CoreJson : JsonSerializerContext
{
    public static JsonTypeInfo<T> Info<T>() =>
        Default.GetTypeInfo(typeof(T)) as JsonTypeInfo<T> ?? throw new CoreException($"{typeof(T).Name} is not listed in CoreJson");

    /// <summary>A value as a node of a call's parameters.</summary>
    public static JsonNode? Node<T>(T value) => JsonSerializer.SerializeToNode(value, Info<T>());
}
