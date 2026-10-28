using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml.Controls;

namespace Clipingo.Views;

public sealed partial class SettingsPage : Page
{
    public SettingsViewModel ViewModel { get; } = App.Services.GetRequiredService<SettingsViewModel>();

    public UpdatesViewModel Updates { get; } = App.Services.GetRequiredService<UpdatesViewModel>();

    public SettingsPage()
    {
        InitializeComponent();
        Loaded += (_, _) => ViewModel.CheckCommand.Execute(null);
    }
}
