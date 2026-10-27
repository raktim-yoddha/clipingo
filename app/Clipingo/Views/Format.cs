using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Clipingo.Views;

/// <summary>x:Bind function helpers that need XAML types (kept out of view models).</summary>
public static class Format
{
    public static ImageSource? Image(string? url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme == Uri.UriSchemeHttps ? new BitmapImage(uri) : null;

    public static Visibility Not(bool value) => value ? Visibility.Collapsed : Visibility.Visible;

    public static string ToolState(bool found, bool required) => found ? "Installed" : required ? "Missing (required)" : "Missing (optional)";

    public static string EncoderState(bool works) => works ? "Works" : "Unavailable";

    public static string InstallName(string label) => $"Install {label}";

    public static Visibility Has(string? text) => string.IsNullOrEmpty(text) ? Visibility.Collapsed : Visibility.Visible;

    public static string ClipLine(double duration, double removed, string encoder) =>
        $"  {duration:0.0}s" + (removed > 0.05 ? $", {removed:0.0}s dead air cut" : "") + $" · {encoder}";
}
