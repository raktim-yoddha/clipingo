using Clipingo.ViewModels;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Clipingo.Views;

public sealed partial class LibraryPage : Page
{
    public LibraryViewModel ViewModel { get; } = App.Services.GetRequiredService<LibraryViewModel>();

    public LibraryPage()
    {
        InitializeComponent();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e) => ViewModel.RefreshCommand.Execute(null);
}
