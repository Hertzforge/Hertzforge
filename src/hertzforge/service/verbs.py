"""The words and modules of the action table, in one place to edit.

Every operation's title stays for its form and the palette; `VERBS` is what
its row in the table says, in two words at most, since the table's column
already says what it acts on. `HIDDEN` lists the operations that have no
row: what a selection draws by itself (a default row names it), a cell typed
into a table, or an act the table offers as its own row.

`MODULES` narrows an operation offered everywhere to the modules whose work
needs it (Brandon, 2026-11-06: Averaging belongs to Signals, Random and
Modal, not Shock or Sine). `PARTS` puts the operations on one part of a
geometry on that part in the tree, so a geometry alone is not offered them.
"""

from __future__ import annotations

from typing import Any

#: the analysis modules, which share one host and one table
STUDIES = ('signals', 'sysid', 'random', 'shock', 'transient', 'sine', 'modal', 'geometry',
           'model', 'specify')
_RECORDED = ('signals', 'sysid', 'random', 'shock', 'transient', 'sine', 'modal')

HIDDEN = frozenset(f'project.{name}' for name in (
    # what a selection draws by itself
    'plot', 'waterfall', 'view_geometry', 'mode_shapes', 'mac', 'dof_arrows', 'spec_spectra',
    'psd_overlay', 'srs_spectra', 'replication_waveforms', 'table', 'channel_table_view',
    'shape_values',
    # typed into a table
    'set_entity', 'set_entity_cell', 'set_channel_cell', 'describe_mode',
    # dragged on the plot
    'set_band_edge',
    # the table's own rows
    'link', 'unlink', 'set_basis', 'name_object_group',
))

VERBS = {f'project.{name}': verb for name, verb in {
    'compute_psds': 'Compute PSDs', 'compute_frfs': 'Compute FRFs', 'fit_modes': 'Fit Modes',
    'integrate': 'Integrate', 'differentiate': 'Differentiate', 'filter_data': 'Apply Filter',
    'truncate_data': 'Apply Truncation', 'resample': 'Resample',
    'compute_spectra': 'Compute Spectra', 'compute_cpsds': 'Compute CPSDs',
    'compute_srs': 'Compute SRS', 'compute_multiple_coherence': 'Compute Coherence',
    'compute_octave': 'Compute Octaves', 'make_realizable': 'Make Realizable',
    'convert_to_specification': 'Make Spec', 'convert_to_psd': 'Make PSD',
    'detect_shocks': 'Detect Shocks', 'extract_sine': 'Extract Sine', 'transform': 'Transform',
    'expand': 'Expand', 'generate_rigid_body_modes': 'Rigid Modes', 'solve_modes': 'Solve',
    'project_onto_basis': 'Project', 'set_averaging': 'Averaging',
    'detect_averaging': 'Detect Averaging', 'set_filter': 'Set Filter',
    'set_truncation': 'Set Truncation', 'group_properties': 'Set Properties',
    'merge_coincident_nodes': 'Merge Nodes', 'element_groups': 'Show Groups',
    'merge_element_groups': 'Merge Groups', 'delete_element_groups': 'Delete Groups',
    'tie_elements': 'Tie', 'align_meshes': 'Align', 'derive_specification': 'Derive Spec',
    'rms_check': 'Check RMS', 'time_statistics': 'Statistics', 'overload': 'Check Overload',
    'spec_rms_error': 'RMS Error', 'spec_lines_out': 'Lines Out', 'spec_chance': 'Chance',
    'spec_composite': 'Composite', 'comparison_scale': 'Scale', 'spec_rms': 'RMS',
    'signal_to_noise': 'SNR', 'rms_signal_to_noise': 'RMS SNR', 'signature': 'Signature',
    'matrix_comparison': 'Matrix', 'srs_error': 'SRS Error', 'srs_points': 'Points Rule',
    'waveform_error': 'Waveform Error', 'replication_segments': 'Segments',
    'save_as': 'Export', 'rattlesnake_preview': 'Preview Run',
    'import_rattlesnake_part': 'Import Part', 'truncate_rattlesnake_file': 'Trim Run',
    'text_preview': 'Preview Text', 'import_text': 'Import Text', 'generate_signal': 'Generate',
    'srs_synthesis': 'Synthesize', 'compute_modal_responses': 'Modal Responses',
    'sweep_rate': 'Sweep Rate', 'fatigue': 'Fatigue', 'drive_prediction': 'Predict Drive',
    'notch_prediction': 'Predict Notch', 'conditioning': 'Conditioning',
    'realizability': 'Realizability', 'view_entities': 'Show Entities', 'add_node': 'Add Node',
    'add_coordinate_system': 'Add System', 'add_element': 'Add Element',
    'add_element_group': 'Add Group', 'delete_entities': 'Delete Picks',
    'rotate_coordinate_system': 'Rotate', 'slide_coordinate_system': 'Slide',
    'reset_rotation': 'Reset Rotation', 'sensors': 'Sensors',
    'work_up_sysid': 'Work Up', 'work_up_random': 'Work Up', 'work_up_shock': 'Work Up',
    'work_up_transient': 'Work Up', 'work_up_sine': 'Work Up', 'work_up_modal': 'Work Up',
    'set_channel_role': 'Channel Role', 'rename_dof': 'Rename DOF',
    'set_control': 'Set Control', 'place': 'Place', 'set_role': 'Group Role',
    'kurtosis': 'Kurtosis', 'amplitude_spectrum': 'Amplitude', 'resample_reading': 'Resample View',
    'plan_instrumentation': 'Plan Sensors', 'export_state_space': 'Export Model',
    'project_onto_channels': 'Project Channels', 'projection_overlay': 'Animate Projection',
    'merge': 'Merge', 'report_features': 'Features', 'report_requirement': 'Requirement',
    'report_finding': 'Finding', 'draft_finding_fact': 'Draft Fact',
    'draft_finding_from_requirement': 'Draft Requirement',
    'draft_findings_from_run_log': 'Draft Findings', 'accept_finding': 'Accept Draft',
    'report_check_block': 'Check Block', 'report_check': 'Check', 'report_test_date': 'Test Dates',
    'report_calibration': 'Calibration', 'report_prediction': 'Prediction',
    'report_run_identity': 'Runs', 'report_runs': 'All Runs', 'report_chronology': 'Chronology',
    'accept_chronology': 'Accept Chronology', 'report_flash': 'Flash Report',
    'index_file': 'Attach File', 'unindex_file': 'Unindex File', 'verify_data_index': 'Verify Index',
    'export_report_template': 'Save Template', 'generate_from_template': 'From Template',
    'reports_from_runs': 'From Runs', 'attach_run_log': 'Attach Log', 'refresh_run_log': 'Refresh Log',
    'detach_run_log': 'Detach Log', 'index_run_log_files': 'Index Log', 'report_identity': 'Identity',
    'amend_report': 'Amend', 'new_channel_table': 'New Table', 'add_channels': 'Add Channel',
    'set_sensor_model': 'Sensor Model', 'calibration_check': 'Calibration',
    'create_specification': 'Create Spec', 'edit_specification': 'Edit Spec',
    'set_breakpoint': 'Breakpoint', 'add_breakpoint': 'Add Breakpoint',
    'remove_breakpoint': 'Remove Breakpoint', 'scale_specification': 'Scale Spec',
    'specification_areas': 'Areas', 'predict': 'Predict', 'sine_extraction': 'Extract Levels',
    'level_error': 'Level Error', 'sine_distortion': 'Distortion', 'sine_frequency': 'Frequency',
    'dwell': 'Dwell', 'mixed_mode': 'Data Sheet', 'pulse_check': 'Check Pulse', 'stroke': 'Stroke',
    'replication_fit': 'Fit', 'wavelet': 'Wavelet', 'set_shock': 'Mark Shock',
    'remove_shock': 'Unmark Shock', 'shocks': 'Shocks', 'new_geometry': 'New Geometry',
    'solids': 'Show Solids', 'add_solid_block': 'Add Block', 'add_solid_cylinder': 'Add Cylinder',
    'add_solid_cone': 'Add Cone', 'add_solid_sphere': 'Add Sphere', 'add_solid_section': 'Add Section',
    'add_solid_plate': 'Add Plate', 'add_solid_disk': 'Add Disk', 'add_solid_hole': 'Add Hole',
    'edit_solid': 'Edit Solid', 'move_solids': 'Move Part', 'copy_solids': 'Copy Solids',
    'reorder_solids': 'Reorder', 'combine_solids': 'Combine', 'separate_solids': 'Take Apart',
    'delete_solids': 'Delete Solids', 'pattern_solids': 'Pattern', 'mesh_solids': 'Mesh',
    'mate_surfaces': 'Mate', 'edit_mated_surfaces': 'Edit Mates', 'edit_mate_sides': 'Edit Sides',
    'delete_mated_surfaces': 'Delete Mates', 'set_view': 'Set View', 'tone_readout': 'Tones',
    'transmissibility': 'Transmissibility', 'confidence_band': 'Confidence',
    'partial_coherence': 'Partial Coherence', 'rigid_body_residual': 'Rigid Residual',
    'field_limit': 'Field Limit', 'octave_bands': 'Octaves', 'sine_profile': 'Profile',
    'cmif': 'CMIF', 'drive_points': 'Drive Points', 'paired_stage': 'Paired Stage',
    'spec_stage': 'Spec Stage', 'match_modes': 'Match Modes', 'add_matches': 'Add Matches',
    'delete_matches': 'Delete Matches',
}.items()}

MODULES: dict[str, tuple[str, ...]] = {f'project.{name}': modules for name, modules in {
    'set_averaging': ('signals', 'sysid', 'random', 'modal'),
    'detect_averaging': ('signals', 'sysid', 'random', 'modal'),
    'set_filter': _RECORDED,
    'set_truncation': _RECORDED,
    'set_channel_role': (*_RECORDED, 'geometry'),
    'rename_dof': (*_RECORDED, 'geometry'),
    'set_control': ('geometry', 'random', 'shock', 'transient', 'sine'),
    'compute_psds': ('sysid', 'random', 'transient'),
    'compute_frfs': ('sysid', 'modal'),
    'compute_multiple_coherence': ('sysid', 'random'),
    'conditioning': ('sysid', 'modal'),
    'wavelet': ('shock', 'transient'),
    'rattlesnake_preview': ('signals',),
    'import_rattlesnake_part': ('signals',),
    'truncate_rattlesnake_file': ('signals',),
    'text_preview': ('signals',),
    'import_text': ('signals',),
}.items()}

#: the part of a geometry in the tree each of these operations belongs to:
#: a part's kind, or 'part:<id>' for the nodes and the coordinate systems. A
#: part picked offers its own operations alone, and the geometry's row none
#: of these: the shapes to build from on the Solids row, as Visual Dynamics
#: offered them (Brandon, 2026-10-11), and what ties or merges parts on the
#: element groups picked together, never the whole geometry
PARTS = {f'project.{name}': part for name, part in {
    'add_solid_block': 'solids',
    'add_solid_cylinder': 'solids',
    'add_solid_cone': 'solids',
    'add_solid_sphere': 'solids',
    'add_solid_section': 'solids',
    'add_solid_plate': 'solids',
    'add_solid_disk': 'solids',
    'solids': 'solids',
    'mesh_solids': 'solids',
    'add_element': 'family',
    'add_element_group': 'family',
    'group_properties': 'group',
    'merge_coincident_nodes': 'group',
    'align_meshes': 'group',
    'mate_surfaces': 'group',
    'add_node': 'part:nodes',
    'add_coordinate_system': 'part:coordinate_systems',
    'rotate_coordinate_system': 'part:coordinate_systems',
    'slide_coordinate_system': 'part:coordinate_systems',
    'reset_rotation': 'part:coordinate_systems',
    'edit_solid': 'solid',
    'move_solids': 'solid',
    'separate_solids': 'solid',
    'delete_solids': 'solid',
    'pattern_solids': 'solid',
    'reorder_solids': 'solids',
    'combine_solids': 'solids',
    'merge_element_groups': 'group',
    'delete_element_groups': 'group',
    'edit_mated_surfaces': 'mate',
    'edit_mate_sides': 'mate',
    'delete_mated_surfaces': 'mate',
}.items()}

#: operations kept off the action table, though they stay operations: a hole
#: is a cylinder to subtract (Brandon, 2026-10-20), and an object is placed in
#: a group by dragging it there in the tree (Brandon, 2026-11-10)
UNOFFERED = frozenset({'project.add_solid_hole', 'project.place'})

#: the operations on element groups that tie or merge two or more of them
TWO_GROUPS = frozenset({'project.merge_element_groups', 'project.align_meshes',
                        'project.mate_surfaces'})


def modules_of(op: Any) -> tuple[str, ...]:
    """The modules that offer `op`: its list here, else every analysis module
    when it says `everywhere`, else its own."""
    if op.id in MODULES:
        return MODULES[op.id]
    return STUDIES if op.everywhere else (op.workspace,)


def part_key(part: Any) -> str:
    """A tree part as `PARTS` names it; '' for none."""
    if not part or not part.get('kind'):
        return ''
    return f'part:{part.get("id")}' if part['kind'] == 'part' else str(part['kind'])


def picked_ids(part: Any) -> list[Any]:
    """Every id of the part picked: the element groups picked together
    (`ids`), else the one (`id`)."""
    if not part:
        return []
    ids = part.get('ids')
    return list(ids) if isinstance(ids, list) and ids else [part.get('id')]
