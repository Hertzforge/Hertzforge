"""sds-analysis: the suite's analysis service, visualdynamics behind sds/1.

Headless before anything else: visualdynamics brings Qt and PyVista, and
the service must never open a window. `setdefault`, so a caller that chose
a platform keeps it.
"""

import os

os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
os.environ.setdefault('PYVISTA_OFF_SCREEN', 'true')
