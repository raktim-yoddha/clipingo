using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Clipingo.ViewModels;

/// <summary>One selectable tile (format, framing, speed, length, caption style…).</summary>
public partial class OptionItem(string id, string title, string mono = "", string hint = "") : ObservableObject
{
    public string Id { get; } = id;
    public string Title { get; } = title;
    /// <summary>Monospace detail next to or above the title ("9:16", "1.5×", "10–30s").</summary>
    public string Mono { get; } = mono;
    public string Hint { get; } = hint;
    /// <summary>Caption preview: font, base and highlight colours.</summary>
    public string Font { get; init; } = "Segoe UI";
    public string Primary { get; init; } = "#FFFFFF";
    public string Highlight { get; init; } = "#FFFFFF";
    public bool Italic { get; init; }
    public bool Upper { get; init; }
    public string Sample => Upper ? "WORDS THAT " : "Words that ";
    public string SampleActive => Upper ? "POP" : "pop";

    internal OptionGroup? Group { get; set; }

    [ObservableProperty]
    public partial bool IsSelected { get; set; }

    partial void OnIsSelectedChanged(bool value) => Group?.Changed(this, value);
}

/// <summary>Tiles that behave as a radio group (one always selected) or as independent toggles.</summary>
public sealed partial class OptionGroup
{
    private bool _updating;

    public OptionGroup(bool multi, params OptionItem[] items)
    {
        Multi = multi;
        Items = new ObservableCollection<OptionItem>(items);
        foreach (var item in items)
            item.Group = this;
        if (!multi && items.Length > 0)
            items[0].IsSelected = true;
    }

    public bool Multi { get; }
    public ObservableCollection<OptionItem> Items { get; }
    public event Action? SelectionChanged;

    public string Selected => Items.FirstOrDefault(i => i.IsSelected)?.Id ?? "";
    public OptionItem? SelectedItem => Items.FirstOrDefault(i => i.IsSelected);
    public IEnumerable<OptionItem> SelectedItems => Items.Where(i => i.IsSelected);

    public void Select(string id)
    {
        if (Items.FirstOrDefault(i => i.Id == id) is { } item)
            item.IsSelected = true;
    }

    public void Replace(IEnumerable<OptionItem> items, string selected)
    {
        Items.Clear();
        foreach (var item in items)
        {
            item.Group = this;
            Items.Add(item);
        }
        Select(selected);
        if (!Multi && SelectedItem is null && Items.Count > 0)
            Items[0].IsSelected = true;
    }

    internal void Changed(OptionItem item, bool selected)
    {
        if (_updating)
            return;
        _updating = true;
        if (!Multi && selected)
        {
            foreach (var other in Items.Where(i => i != item))
                other.IsSelected = false;
        }
        else if (!Multi && !selected)
        {
            item.IsSelected = true; // a radio group can't be emptied by clicking the selected tile
        }
        _updating = false;
        SelectionChanged?.Invoke();
    }
}
