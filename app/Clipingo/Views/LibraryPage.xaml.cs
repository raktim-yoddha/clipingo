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

    private async void Delete_Click(object sender, Microsoft.UI.Xaml.RoutedEventArgs e)
    {
        if ((sender as Microsoft.UI.Xaml.FrameworkElement)?.Tag is not RunCard card)
            return;
        var confirm = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Remove this video from the Library?",
            Content = $"“{card.Title}”, its transcript and clip list are deleted. Clips you exported stay in {card.Folder}.",
            PrimaryButtonText = "Remove",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await confirm.ShowAsync() == ContentDialogResult.Primary)
            await ViewModel.DeleteAsync(card);
    }

    private void Edit_Click(object sender, Microsoft.UI.Xaml.RoutedEventArgs e) =>
        App.MainWindow?.OpenEditor((string)((Microsoft.UI.Xaml.FrameworkElement)sender).Tag, null);
}
