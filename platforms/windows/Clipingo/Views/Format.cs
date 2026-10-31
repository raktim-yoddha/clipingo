using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.UI.Text;

namespace Clipingo.Views;

/// <summary>x:Bind function helpers that need XAML types (kept out of view models).</summary>
public static class Format
{
    public static ImageSource? Image(string? url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme == Uri.UriSchemeHttps ? new BitmapImage(uri) : null;

    /// <summary>A local image file (thumbnails), or null.</summary>
    public static ImageSource? File(string? path) => string.IsNullOrEmpty(path) ? null : new BitmapImage(new Uri(path));

    public static Visibility Not(bool value) => value ? Visibility.Collapsed : Visibility.Visible;

    public static Visibility Both(bool a, bool b) => a && b ? Visibility.Visible : Visibility.Collapsed;

    public static double Dim(bool enabled) => enabled ? 1 : 0.35;

    public static string ToolState(bool found, bool required) => found ? "Installed" : required ? "Missing (required)" : "Missing (optional)";

    public static string EncoderState(bool works) => works ? "Works" : "Unavailable";

    public static string InstallName(string label) => $"Install {label}";

    /// <summary>Accessible tile name: "Extra short 10–30s", "1.5×", "Caption style Pop".</summary>
    public static string TileName(string a, string b) => string.IsNullOrEmpty(a) ? b : string.IsNullOrEmpty(b) ? a : $"{a} {b}";

    public static string Either(string a, string b) => string.IsNullOrEmpty(a) ? b : a;

    public static bool HasText(string? text) => !string.IsNullOrEmpty(text);

    public static Visibility Has(string? text) => string.IsNullOrEmpty(text) ? Visibility.Collapsed : Visibility.Visible;

    public static Visibility Lacks(string? text) => string.IsNullOrEmpty(text) ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>A star column width (the editor's timeline band).</summary>
    public static GridLength Stars(double value) => new(Math.Max(0.001, value), GridUnitType.Star);

    public static string ClipLine(double duration, double removed, string encoder) =>
        $"  {duration:0.0}s" + (removed > 0.05 ? $", {removed:0.0}s dead air cut" : "") + $" · {encoder}";

    // ── Wizard stepper: active step = accent dot on a subtle pill, done = filled dot with a check ──

    /// <summary>A Fluent theme brush (XamlControlsResources), for the current theme.</summary>
    private static Brush Res(string key) => (Brush)Application.Current.Resources[key];

    public static Visibility IsStep(int step, int i) => step == i ? Visibility.Visible : Visibility.Collapsed;

    public static Brush StepDot(int step, int i) =>
        Res(i == step ? "AccentFillColorDefaultBrush" : i < step ? "TextFillColorPrimaryBrush" : "ControlFillColorDefaultBrush");

    public static Brush StepDotInk(int step, int i) =>
        Res(i == step ? "TextOnAccentFillColorPrimaryBrush" : i < step ? "SolidBackgroundFillColorBaseBrush" : "TextFillColorPrimaryBrush");

    public static string StepNumber(int step, int i) => i < step ? "" : (i + 1).ToString();

    public static FontFamily StepNumberFont(int step, int i) =>
        new(i < step ? "Segoe Fluent Icons, Segoe MDL2 Assets" : "Segoe UI Variable Text, Segoe UI");

    public static Brush StepLabel(int step, int i) => Res(i == step ? "TextFillColorPrimaryBrush" : "TextFillColorSecondaryBrush");

    public static Brush StepPill(int step, int i) => i == step ? Res("SubtleFillColorSecondaryBrush") : new SolidColorBrush(Colors.Transparent);

    public static Brush StepLine(int step, int i) => Res(i < step ? "TextFillColorTertiaryBrush" : "DividerStrokeColorDefaultBrush");

    // ── Tiles ──

    public static Visibility Is(string id, string value) => id == value ? Visibility.Visible : Visibility.Collapsed;

    public static double RatioWidth(string id) => id == "vertical" ? 12 : 24;

    public static double RatioHeight(string id) => id == "vertical" ? 21 : 14;

    public static Brush Hex(string hex)
    {
        var h = hex.TrimStart('#');
        byte P(int at) => Convert.ToByte(h.Substring(at, 2), 16);
        return h.Length == 6 ? new SolidColorBrush(ColorHelper.FromArgb(0xFF, P(0), P(2), P(4))) : Res("TextFillColorPrimaryBrush");
    }

    public static FontFamily Font(string name) => new(name);

    public static FontStyle Italic(bool italic) => italic ? FontStyle.Italic : FontStyle.Normal;

    /// <summary>Paper's preview sits on a white card; the rest on dark footage.</summary>
    public static Brush PreviewBackground(string id) =>
        id == "paper" ? new SolidColorBrush(Colors.White) : new SolidColorBrush(ColorHelper.FromArgb(0xFF, 0x0B, 0x0C, 0x10));

    public static Brush StatusBrush(bool done, bool failed) =>
        Res(done ? "SystemFillColorSuccessBrush" : failed ? "SystemFillColorCriticalBrush" : "AccentTextFillColorPrimaryBrush");
}
