using Microsoft.UI.Composition;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Clipingo.Views;

/// <summary>
/// The window's main surface: Mica in one neutral gray, focused or not (ADR 0005). WinUI's <see cref="MicaBackdrop"/> is
/// tinted by the wallpaper while the window is active and turns a flat fill once you click elsewhere. This one always
/// counts as active, and its tint is strong enough that nothing behind the app colours it.
/// </summary>
public sealed partial class GrayBackdrop : SystemBackdrop
{
    private readonly SystemBackdropConfiguration _config = new() { IsInputActive = true };
    private MicaController? _mica;

    protected override void OnTargetConnected(ICompositionSupportsSystemBackdrop connectedTarget, XamlRoot xamlRoot)
    {
        base.OnTargetConnected(connectedTarget, xamlRoot);
        _mica = new MicaController { TintOpacity = 0.95f, LuminosityOpacity = 1f };
        if (xamlRoot.Content is FrameworkElement root)
        {
            Paint(root.ActualTheme);
            root.ActualThemeChanged += (sender, _) => Paint(sender.ActualTheme);
        }
        else
        {
            Paint(Application.Current.RequestedTheme == ApplicationTheme.Light ? ElementTheme.Light : ElementTheme.Dark);
        }

        _mica.AddSystemBackdropTarget(connectedTarget);
        _mica.SetSystemBackdropConfiguration(_config);
    }

    protected override void OnTargetDisconnected(ICompositionSupportsSystemBackdrop disconnectedTarget)
    {
        base.OnTargetDisconnected(disconnectedTarget);
        _mica?.RemoveSystemBackdropTarget(disconnectedTarget);
        _mica?.Dispose();
        _mica = null;
    }

    private void Paint(ElementTheme theme)
    {
        if (_mica is null)
        {
            return;
        }

        var light = theme == ElementTheme.Light;
        _config.Theme = light ? SystemBackdropTheme.Light : SystemBackdropTheme.Dark;
        _mica.TintColor = _mica.FallbackColor = light ? Color.FromArgb(255, 0xF3, 0xF3, 0xF3) : Color.FromArgb(255, 0x2C, 0x2C, 0x2C);
    }
}
