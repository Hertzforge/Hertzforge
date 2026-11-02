"""Drawing decisions from Visual Dynamics' `theme`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

LIGHT = {
    'name': 'light',
    'scene_background': '#ffffff',
    # flat, equal to the base: the gradient read as depth for a while
    # and then read as clutter (Brandon, 2026-08-23) — pure white and
    # pure black let the data carry the scene
    'scene_background_top': '#ffffff',
    'scene_text': '#1c1c1e',
    'plot_background': '#ffffff',
    'plot_background_top': '#ffffff',
    'plot_foreground': '#1c1c1e',
    'scene_muted': '#b8b8bd',       # context geometry behind a selection
    'scene_highlight': '#ff8c2b',   # the selected entity, or the one hovered
    'scene_picked': '#1f9d55',      # nodes gathered so far for a new element
    'scene_merged': '#2f6fd0',      # places Merge Coincident Nodes has joined
    'scene_solid': '#c08a3e',       # solids, before they are meshed
    'row_error': '#c02a1f',         # an object that does not fit the geometry
    # the zones a random-vibration response must not be in: between the
    # warning and abort limits, and beyond abort. Filled translucent, so
    # these are the colors before alpha.
    'limit_warning': '#e6b800',
    'limit_abort': '#d13b2e',
    # the frames a PSD will be averaged over, shaded on the time
    # history they are cut from. The band is translucent, so that is the
    # color before alpha; the window drawn over each frame is warm on
    # purpose — a blue mark over blue traces reads as another channel
    # a measured line that went outside an abort limit, shaded over its
    # own bin: red where it went over, blue where it fell under. Two
    # colors because which way it went is the first thing to know,
    # and stronger than the zone shading they sit inside
    # a specification and the response it bounds, drawn as a pair: the
    # response in the foreground color because it is what is being
    # looked at, the specification in gray behind it because it is the
    # reference. Neither takes a palette color — there is only ever
    # one pair on the plot, so there is nothing to tell apart.
    'response_curve': '#1c1c1e',
    'specification_curve': '#8a8a8f',
    'exceed_over': '#d13b2e',
    'exceed_under': '#2f6fd0',
    'averaging_band': '#3b7dd8',
    'averaging_window': '#c2410c',
    # the filtered trace previewed over the raw one, flat and on the
    # stage alike. **Magenta, measured** (Brandon, 2026-08-25): green
    # was the first choice against the 2-D palette, and it disappeared
    # on the stage — viridis runs purple, teal, *green*, yellow, so a
    # green twin sat inside the very colormap it had to stand out
    # from (49 units from the nearest stop, against magenta's 203).
    # Magenta is in neither the colormap nor the mark colors.
    'filter_preview': '#b5179e',
}


DARK = {
    'name': 'dark',
    'scene_background': '#000000',
    # flat, equal to the base — see the light theme's note
    'scene_background_top': '#000000',
    'scene_text': '#e8e8ea',
    'plot_background': '#000000',
    'plot_background_top': '#000000',
    'plot_foreground': '#e8e8ea',
    'scene_muted': '#4a4a55',       # context geometry behind a selection
    'scene_highlight': '#ffb020',   # the selected entity, or the one hovered
    'scene_picked': '#3ddc84',      # nodes gathered so far for a new element
    'scene_merged': '#5b9bff',      # places Merge Coincident Nodes has joined
    'scene_solid': '#e0a85a',       # solids, before they are meshed
    'row_error': '#ff6b61',         # an object that does not fit the geometry
    # the zones a random-vibration response must not be in: between the
    # warning and abort limits, and beyond abort. Filled translucent, so
    # these are the colors before alpha.
    'limit_warning': '#ffcc33',
    'limit_abort': '#ff6b61',
    # the frames a PSD will be averaged over, shaded on the time
    # history they are cut from. The band is translucent, so that is the
    # color before alpha; the window drawn over each frame is warm on
    # purpose — a blue mark over blue traces reads as another channel
    # a measured line that went outside an abort limit, shaded over its
    # own bin: red where it went over, blue where it fell under. Two
    # colors because which way it went is the first thing to know,
    # and stronger than the zone shading they sit inside
    # a specification and the response it bounds, drawn as a pair: the
    # response in the foreground color because it is what is being
    # looked at, the specification in gray behind it because it is the
    # reference. Neither takes a palette color — there is only ever
    # one pair on the plot, so there is nothing to tell apart.
    'response_curve': '#ffffff',
    'specification_curve': '#9a9aa2',
    'exceed_over': '#ff6b61',
    'exceed_under': '#5fa8ff',
    'averaging_band': '#5fa8ff',
    'averaging_window': '#fb923c',
    'filter_preview': '#ff5ae0',
}


#: How solid the *other* set is when two mode shapes are overlaid. The
#: basis stays opaque and the comparison is drawn through it: a finite
#: element model has a skin where a test set has a wireframe, and an
#: opaque FEM simply hides the thing it is being compared against.
#:
#: Here rather than in the window because the report draws the same
#: overlay in a canvas and must draw it the same way. It did not, and
#: the difference was invisible until the two were put side by side.
OVERLAY_ALPHA = 0.25


#: an overlaid pair's two flat colors when nothing else chooses them —
#: blue, the Basis bracket's own, and orange: the report's overlay, a
#: script's, and the window's for a set no group colors
PAIR_COLORS = ('#4c92d9', '#ff8c2b')


def overlay_alphas(first_role: str | None,
                   second_role: str | None) -> tuple[float, float]:
    """The (first, second) opacity of an overlaid pair of shape sets.

    The set whose role is 'Basis' is opaque and the other is drawn at
    `OVERLAY_ALPHA` over it, whichever order the pair was picked in.
    With no Basis, or two, the first set is the opaque one.
    """
    if second_role == 'Basis' and first_role != 'Basis':
        return OVERLAY_ALPHA, 1.0
    return 1.0, OVERLAY_ALPHA


#: viridis, as the nine stops everything interpolates between. One
#: color scale, one set of numbers: the app paints a moving model by
#: displacement on it, pyqtgraph draws its MAC grids and coherence
#: maps on it, the app icon is drawn with it, and the report is handed
#: these very stops rather than carrying a second copy in JavaScript —
#: so a color means one thing wherever it appears.
VIRIDIS = ((68, 1, 84), (71, 44, 122), (59, 81, 139), (44, 113, 142),
           (33, 144, 141), (39, 173, 129), (92, 200, 99),
           (170, 220, 50), (253, 231, 37))
