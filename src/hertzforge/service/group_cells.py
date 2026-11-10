"""An element group's properties as cells of the Element Groups table.

The properties a finite element model needs (`fem.GroupProperties`), one
column each: a material, picked from the library or typed
number by number; a thickness for plates; a section and an orientation
for beams, by a shape and its dimensions or by its four numbers; a mass
for points; a stiffness for points and lines, which makes them springs;
the directions held for points, which makes them ground. Values are
stored and typed in SI. A geometry whose units are not declared holds
its own numbers, and the headings say so.
"""

from __future__ import annotations

import math
from dataclasses import replace
from typing import Any

from .parsing import parse_numbers

#: every column: key, title, what holds the value ('' for the property set
#: itself), the field there, and the SI unit its heading names
COLUMNS = (
    ('material', 'Material', 'material', 'name', ''),
    ('modulus', 'E', 'material', 'youngs_modulus', 'Pa'),
    ('poisson', 'ν', 'material', 'poissons_ratio', ''),
    ('density', 'ρ', 'material', 'density', 'kg/m³'),
    ('thickness', 'Thickness', '', 'thickness', 'm'),
    ('section', 'Section', 'section', 'name', ''),
    ('shape', 'Shape', 'section', 'shape', ''),
    ('dimensions', 'Dimensions', 'section', 'dimensions', 'm'),
    ('area', 'A', 'section', 'area', 'm²'),
    ('iy', 'Iy', 'section', 'iy', 'm⁴'),
    ('iz', 'Iz', 'section', 'iz', 'm⁴'),
    ('j', 'J', 'section', 'j', 'm⁴'),
    ('orientation', 'Orientation', '', 'orientation', ''),
    ('mass', 'Mass', '', 'mass', 'kg'),
    ('stiffness', 'Stiffness', '', 'stiffness', 'N/m, N·m/rad'),
    ('ground', 'Ground', '', 'ground', ''),
)
KEYS = tuple(column[0] for column in COLUMNS)
_BY_KEY = {column[0]: column for column in COLUMNS}

#: the Material list's entry that makes a group of points ground
GROUND = 'ground (fixed)'
CUSTOM = 'custom'
STIFFNESS_LABELS = ('Kx', 'Ky', 'Kz', 'Krx', 'Kry', 'Krz')
#: the short names a shape's dimensions are written with, in the
#: constructor's order (`fem.SHAPES`)
SHAPE_LABELS = {
    'round tube': ('D', 't'), 'rod': ('D',), 'rectangle': ('b', 'h'),
    'rectangular tube': ('b', 'h', 't'),
    'I-beam': ('d', 'bf', 'tf', 'tw'), 'channel': ('d', 'bf', 'tf', 'tw'),
    'angle': ('a', 'b', 't'),
}

#: the element families a property means something for; a material is for all
_FAMILIES = {'thickness': ('face',), 'mass': ('point',), 'ground': ('point',),
             'stiffness': ('point', 'line'), 'orientation': ('line',)}
_WORDS = {'face': 'plates', 'line': 'beams', 'point': 'points', 'volume': 'solids'}


def titles(geometry: Any) -> list[str]:
    """The columns' headings, naming SI, or saying the numbers are the
    file's own while the geometry's units are not declared."""
    out = []
    for _key, title, _holder, _field, unit in COLUMNS:
        if not unit:
            out.append(title)
        else:
            out.append(f'{title} [{unit if geometry.units_defined else "deck units"}]')
    return out


def choices() -> dict[str, list[str]]:
    """What the Material and Shape cells offer. The materials are a
    shortlist: any other name is a name, its numbers typed beside it."""
    from hertzforge.core.fem import MATERIALS, SHAPES

    return {'material': [*MATERIALS, GROUND], 'shape': [*SHAPES, CUSTOM]}


def family(geometry: Any, group: int) -> str:
    """'face', 'volume', 'line' or 'point': what an element group's
    elements are; 'mixed' when they differ, '' when it has none."""
    from hertzforge.core.geometry import ELEMENT_TYPES

    ids = set(geometry.elements_in(group))
    families = {ELEMENT_TYPES.get(int(kind), ('', 0, ''))[2]
                for elem, kind in zip(geometry.elem_id, geometry.elem_type, strict=True)
                if int(elem) in ids}
    if not families:
        return ''
    return families.pop() if len(families) == 1 else 'mixed'


def _why_not(geometry: Any, group: int, key: str) -> str:
    """Why a group's elements do not use this property, or ''."""
    holder = _BY_KEY[key][2]
    wanted = ('line',) if holder == 'section' else _FAMILIES.get(key)
    held = family(geometry, group)
    if wanted is None or not held or held in wanted or held == 'mixed':
        return ''
    word = 'section' if holder == 'section' else key
    if held == 'volume':
        return f'a group of solids takes a material alone: no {word}'
    takers = ' and '.join(_WORDS[f] for f in wanted)
    return f'a group of {_WORDS[held]} takes no {word}; {takers} do'


def _text(value: float) -> str:
    return f'{value:.12g}'


def _dimensions_text(section: Any) -> str:
    """'D=0.025, t=0.002', or what a shape wants with nothing typed yet;
    an angle adds where its orientation points."""
    from hertzforge.core.fem import angle_major_axis

    labels = SHAPE_LABELS[section.shape]
    if not section.dimensions:
        return ', '.join(f'{label}=?' for label in labels)
    text = ', '.join(f'{label}={value:.6g}'
                     for label, value in zip(labels, section.dimensions, strict=True))
    if section.shape == 'angle':
        text += f'; orient {angle_major_axis(*section.dimensions):.1f}° from the long leg'
    return text


def cell(geometry: Any, group: int, key: str) -> str:
    """The text of one property cell; blank until it is set."""
    props = geometry.group_properties.get(group)
    if props is None:
        return ''
    _key, _title, holder, field, _unit = _BY_KEY[key]
    if key == 'material' and props.ground:
        return GROUND
    if key == 'stiffness':
        if props.stiffness is None:
            return ''
        return ', '.join(f'{label}={value:.6g}' for label, value
                         in zip(STIFFNESS_LABELS, props.stiffness, strict=True)
                         if value is not None)
    if key == 'ground':
        return '' if not props.ground else 'all' if len(props.ground) == 6 else ', '.join(props.ground)
    owner = getattr(props, holder) if holder else props
    if owner is None:
        return ''
    if holder == 'material' and field != 'name' and owner.is_rigid:
        return ''   # a link has no modulus, density or ratio: blank, not 'inf'
    if key == 'shape':
        return owner.shape or CUSTOM
    if key == 'dimensions':
        return _dimensions_text(owner) if owner.shape else ''
    if key == 'orientation':
        return ', '.join(f'{v:g}' for v in owner.orientation) if owner.orientation else ''
    value = getattr(owner, field)
    if isinstance(value, str):
        return value
    # a number not typed yet is held as nothing or as 0 by what made the row
    return _text(value) if value else ''


def cells(geometry: Any, group: int) -> list[str]:
    """One group's property cells, in `COLUMNS` order."""
    return [cell(geometry, group, key) for key in KEYS]


def _number(text: str, title: str) -> float:
    value = float(text)
    if not math.isfinite(value) or value < 0.0:
        raise ValueError(f'{title} cannot be {text}')
    return value


def _stiffness(text: str) -> tuple[float | None, ...]:
    """Six stiffnesses from 'Kz=1e5, Kry=2e3'; a direction not named is free."""
    values: list[float | None] = [None] * 6
    lower = [label.lower() for label in STIFFNESS_LABELS]
    for token in text.replace(';', ',').split(','):
        token = token.strip()
        if not token:
            continue
        label, eq, number = token.partition('=')
        label = label.strip().lower()
        if not eq or label not in lower:
            raise ValueError('a stiffness is given by direction: '
                             + ', '.join(f'{name}=' for name in STIFFNESS_LABELS)
                             + ' (Kz=1e5, Kry=2e3)')
        k = lower.index(label)
        value = float(number)
        if not (math.isfinite(value) and value > 0.0):
            raise ValueError(f'{STIFFNESS_LABELS[k]} is positive, or left out for a '
                             'direction that is free')
        values[k] = value
    return tuple(values)


def _with_shape(props: Any, key: str, text: str) -> Any:
    """The properties with the section's shape or dimensions changed. A new
    shape starts empty, since its dimensions mean different things; Custom
    keeps the four numbers and lets them be typed. Dimensions are numbers in
    order or the labels shown ('D=0.025, t=0.002'), anything after a ';'
    ignored."""
    from hertzforge.core.fem import SHAPES, Section, with_article

    section = props.section or Section('', 0.0, 0.0, 0.0, 0.0)
    if key == 'shape':
        if text in ('', CUSTOM):
            return replace(props, section=replace(section, shape='', dimensions=()))
        if text not in SHAPES:
            raise ValueError(f'{text!r} is not a section shape: '
                             + ', '.join([*SHAPES, CUSTOM]))
        if text == section.shape:
            return props
        return replace(props, section=Section(section.name, 0.0, 0.0, 0.0, 0.0, text, ()))
    if not section.shape:
        if not text:
            return props
        raise ValueError('choose a shape first, in the Shape column, or type A, Iy, Iz '
                         'and J for a custom section')
    if not text:
        return replace(props, section=Section(section.name, 0.0, 0.0, 0.0, 0.0,
                                              section.shape, ()))
    labels = SHAPE_LABELS[section.shape]
    values = []
    for token in text.split(';')[0].replace(',', ' ').split():
        if '=' in token:
            label, _eq, token = token.partition('=')
            if label.strip() not in labels:
                raise ValueError(f'{with_article(section.shape)} is given as '
                                 + ', '.join(labels))
        values.append(float(token))
    if len(values) != len(labels):
        names = SHAPES[section.shape][1]
        raise ValueError(f'{with_article(section.shape)} takes {len(labels)} dimensions: '
                         + ', '.join(f'{label} ({name})' for label, name
                                     in zip(labels, names, strict=True)))
    return replace(props, section=Section.of_shape(section.name, section.shape, values))


def _hold(geometry: Any, group: int, props: Any) -> None:
    """Keep the properties, unless clearing cells has left them saying
    nothing: an empty section comes off, so a group given one by mistake
    becomes what it was, and a group with nothing left has no properties."""
    section = props.section
    if (section is not None and not (section.name or section.shape or section.dimensions)
            and not any((section.area, section.iy, section.iz, section.j))):
        props = replace(props, section=None)
    material = props.material
    blank = material is None or not (material.name or material.youngs_modulus
                                     or material.density)
    if blank and props.kind in ('no material', 'solid') and props.orientation is None:
        geometry.group_properties.pop(group, None)
    else:
        geometry.group_properties[group] = props


def set_cell(geometry: Any, group: int, key: str, text: str) -> None:
    """One property cell typed; a ValueError says why it was refused. A mass,
    a stiffness or a ground replaces whatever the group was, since each is
    the one thing such a group has, and clearing it clears the group."""
    from hertzforge.core.fem import MATERIALS, GroupProperties, Material

    text = str(text).strip()
    why = _why_not(geometry, group, key)
    if why:
        if not text:
            return
        raise ValueError(why)
    props = geometry.group_properties.get(group)
    kind = props.kind if props is not None else ''
    _key, title, holder, field, _unit = _BY_KEY[key]
    if key == 'mass':
        if text:
            geometry.group_properties[group] = GroupProperties(mass=_number(text, title))
        elif kind == 'mass':
            geometry.group_properties.pop(group, None)
        return
    if key == 'stiffness':
        if text:
            geometry.group_properties[group] = GroupProperties(stiffness=_stiffness(text))
        elif kind == 'spring':
            geometry.group_properties.pop(group, None)
        return
    if key == 'ground' or (key == 'material' and text == GROUND):
        if text:
            held = True if text.lower() == 'all' or text == GROUND else text
            geometry.group_properties[group] = GroupProperties(ground=held)
        elif kind == 'ground':
            geometry.group_properties.pop(group, None)
        return
    if not text and kind in ('mass', 'spring'):
        return
    if kind == 'mass':
        raise ValueError('a group of point masses takes a mass alone: clear its Mass to '
                         'give it a material')
    if kind == 'spring':
        raise ValueError('a group of springs takes a stiffness alone: clear its Stiffness '
                         'to give it a material')
    if kind == 'ground' and not (key == 'material' and text in MATERIALS):
        if not text:
            return
        raise ValueError('a ground group holds its points and takes nothing else: pick a '
                         'material to make it something else')
    if key == 'material' and text in MATERIALS:
        # a library name fills the row; any other name is a name
        geometry.group_properties[group] = replace(
            props or GroupProperties(MATERIALS[text]), material=MATERIALS[text], ground=False)
        return
    if props is None:
        if not text:
            return
        props = GroupProperties(Material('', 0.0, 0.0, 0.3))
    if key in ('shape', 'dimensions'):
        _hold(geometry, group, _with_shape(props, key, text))
        return
    if holder == 'material' and field != 'name' and props.material.is_rigid:
        raise ValueError('a rigid link has no modulus, density or ratio: pick a material '
                         'to give it one')
    section = props.section
    if holder == 'section' and field != 'name' and section is not None and section.shape:
        raise ValueError(f'computed from the {section.shape}\'s dimensions: choose '
                         f'{CUSTOM} in Shape to type it')
    if key == 'orientation':
        parts = tuple(parse_numbers(text, 'an orientation'))
        if text and len(parts) != 3:
            raise ValueError('an orientation is three numbers')
        _hold(geometry, group, replace(props, orientation=parts or None))
        return
    if field == 'name':
        value: Any = text
    elif not text:
        value = None if not holder else 0.0
    elif key == 'poisson':
        value = float(text)
        if not -1.0 < value < 0.5:
            raise ValueError(f'ν is between -1 and 0.5: not {text}')
    else:
        value = _number(text, title)
    if holder == 'material':
        made = replace(props.material or Material('', 0.0, 0.0, 0.3), **{field: value})
        _hold(geometry, group, replace(props, material=made))
    elif holder == 'section':
        from hertzforge.core.fem import Section

        made = replace(section or Section('', 0.0, 0.0, 0.0, 0.0), **{field: value})
        _hold(geometry, group, replace(props, section=made))
    else:
        _hold(geometry, group, replace(props, **{field: value}))
