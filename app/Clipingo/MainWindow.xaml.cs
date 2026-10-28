using Clipingo.Views;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Clipingo;

public sealed partial class MainWindow : Window
{
    private static readonly Dictionary<string, Type> Pages = new()
    {
        ["Create"] = typeof(CreatePage),
        ["Library"] = typeof(LibraryPage),
        ["Jobs"] = typeof(JobsPage),
        ["Settings"] = typeof(SettingsPage),
    };

    public MainWindow()
    {
        InitializeComponent();

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredTheme = TitleBarTheme.UseDefaultAppMode;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        AppWindow.Resize(new Windows.Graphics.SizeInt32(1360, 900));

        Nav.SelectedItem = CreateItem;
    }

    /// <summary>Selects a sidebar page by its tag ("Create", "Library", "Jobs", "Settings").</summary>
    public void Navigate(string tag) =>
        Nav.SelectedItem = Nav.MenuItems.OfType<NavigationViewItem>().First(i => (string)i.Tag == tag);

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is NavigationViewItem { Tag: string tag } && Pages[tag] is var page && ContentFrame.CurrentSourcePageType != page)
            ContentFrame.Navigate(page);
    }
}
