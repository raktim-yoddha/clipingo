using Clipingo.Services;
using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml;

namespace Clipingo;

public partial class App : Application
{
    /// <summary>Composition root; pages resolve their view models here.</summary>
    public static IServiceProvider Services { get; } = new ServiceCollection()
        .AddSingleton<CoreClient>()
        .AddTransient<CreateViewModel>()
        .AddTransient<SettingsViewModel>()
        .BuildServiceProvider();

    public static MainWindow? MainWindow { get; private set; }

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        MainWindow = new MainWindow();
        MainWindow.Activate();
    }
}
