using System.IO;
using Windows.Storage;
using Windows.System;

namespace Clipingo.Services;

/// <summary>Showing files to the user in File Explorer.</summary>
public static class Shell
{
    /// <summary>
    /// Opens a folder in File Explorer, or the folder of a file with the file selected. Goes through the shell's
    /// launcher (the user's own Explorer) instead of starting explorer.exe as our child process.
    /// </summary>
    public static async Task<bool> RevealAsync(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                var options = new FolderLauncherOptions();
                options.ItemsToSelect.Add(await StorageFile.GetFileFromPathAsync(path));
                return await Launcher.LaunchFolderPathAsync(Path.GetDirectoryName(path)!, options);
            }
            return Directory.Exists(path) && await Launcher.LaunchFolderPathAsync(path);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException)
        {
            return false;
        }
    }
}
