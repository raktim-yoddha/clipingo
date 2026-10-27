using System.Diagnostics;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Clipingo.Services;

/// <summary>Error reported by the Rust core (clipd).</summary>
public sealed class CoreException(string message) : Exception(message);

/// <summary>
/// JSON-RPC 2.0 client for clipd over the <c>\\.\pipe\clipingo</c> named pipe, one JSON object per line.
/// Starts clipd on first use if it is not already running.
/// </summary>
public sealed class CoreClient : IAsyncDisposable
{
    private const string PipeName = "clipingo";

    public static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };

    private readonly SemaphoreSlim _gate = new(1, 1);
    private NamedPipeClientStream? _pipe;
    private StreamReader? _reader;
    private StreamWriter? _writer;
    private int _nextId;

    public async Task<T> CallAsync<T>(string method, object? parameters, CancellationToken ct = default)
    {
        await _gate.WaitAsync(ct);
        try
        {
            await EnsureConnectedAsync(ct);
            var id = ++_nextId;
            var request = new JsonObject { ["jsonrpc"] = "2.0", ["id"] = id, ["method"] = method };
            if (parameters is not null)
                request["params"] = JsonSerializer.SerializeToNode(parameters, Json);
            await _writer!.WriteLineAsync(request.ToJsonString().AsMemory(), ct);

            var line = await _reader!.ReadLineAsync(ct) ?? throw new IOException("clipd closed the connection");
            var reply = JsonNode.Parse(line)!.AsObject();
            if (reply["error"] is JsonObject error)
                throw new CoreException(error["message"]?.GetValue<string>() ?? "Unknown core error");
            return reply["result"].Deserialize<T>(Json) ?? throw new CoreException($"{method} returned no result");
        }
        catch (IOException)
        {
            Disconnect(); // next call reconnects (and restarts clipd if it died)
            throw;
        }
        finally
        {
            _gate.Release();
        }
    }

    private async Task EnsureConnectedAsync(CancellationToken ct)
    {
        if (_pipe is { IsConnected: true })
            return;
        Disconnect();
        if (!await TryConnectAsync(500, ct))
        {
            StartDaemon();
            if (!await TryConnectAsync(5000, ct))
                throw new CoreException("Could not reach clipd. Build it with `cargo build` or set CLIPINGO_CLIPD.");
        }
        _reader = new StreamReader(_pipe!, new UTF8Encoding(false));
        _writer = new StreamWriter(_pipe!, new UTF8Encoding(false)) { AutoFlush = true, NewLine = "\n" };
    }

    private async Task<bool> TryConnectAsync(int timeoutMs, CancellationToken ct)
    {
        _pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
        try
        {
            await _pipe.ConnectAsync(timeoutMs, ct);
            return true;
        }
        catch (TimeoutException)
        {
            await _pipe.DisposeAsync();
            _pipe = null;
            return false;
        }
    }

    private static void StartDaemon()
    {
        var exe = FindDaemon() ?? throw new CoreException("clipd.exe not found. Build it with `cargo build` or set CLIPINGO_CLIPD.");
        Process.Start(new ProcessStartInfo(exe) { UseShellExecute = false, CreateNoWindow = true });
    }

    /// <summary>CLIPINGO_CLIPD, next to the app, or the repo's cargo output when running from source.</summary>
    private static string? FindDaemon()
    {
        if (Environment.GetEnvironmentVariable("CLIPINGO_CLIPD") is { Length: > 0 } env && File.Exists(env))
            return env;
        var beside = Path.Combine(AppContext.BaseDirectory, "clipd.exe");
        if (File.Exists(beside))
            return beside;
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            foreach (var profile in new[] { "release", "debug" })
            {
                var candidate = Path.Combine(dir.FullName, "target", profile, "clipd.exe");
                if (File.Exists(candidate))
                    return candidate;
            }
        }
        return null;
    }

    private void Disconnect()
    {
        _reader?.Dispose();
        _writer?.Dispose();
        _pipe?.Dispose();
        (_reader, _writer, _pipe) = (null, null, null);
    }

    public ValueTask DisposeAsync()
    {
        Disconnect();
        _gate.Dispose();
        return ValueTask.CompletedTask;
    }
}
