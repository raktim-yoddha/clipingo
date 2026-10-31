# ADR 0005 — An opaque gray window with acrylic panels

Decided by the product owner, 2026-10-31.

## Context

The window used desktop acrylic, so other windows showed through the whole app. A plain black or silver fill looked like
a web page. The product owner pointed at the Settings window of their own app, Pluto, as the look to match.

## Decision

The window's surface is `GrayBackdrop`: Mica tinted neutral gray (`#2C2C2C` dark, `#F3F3F3` light) at 95 %, always
counted as active, so it looks the same with or without focus. Panels, cards and bars on it are in-app acrylic
(`GlassBrush`, `GlassBarBrush`) a step lighter, with a light top edge. There is no gradient on the surface.

Type, corners and spacing follow Microsoft's published Windows values: the type ramp with Segoe UI Variable, 4 px corners
on controls and 8 px on surfaces (the Fluent defaults, not overridden), 8 between controls, 12 between content areas, 16
from a surface to its text.

## Consequences

Mica lets a trace of the wallpaper's colour through at 5 %; that is the price of a material over a flat fill. With
transparency effects off or on battery saver, the surface and panels fall back to solid colours. In-app acrylic has
nothing to blur on an opaque surface, so panels read through tint, grain and edge rather than see-through.
