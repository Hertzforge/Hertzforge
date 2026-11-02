"""Photos: dropped images, one object per project, and photo figures.

Dropping png/jpeg files on the tree lands them together in the
project's Photos object; a report's photo block embeds one of them as
a numbered figure, chosen by name from a drop-down.
"""

from __future__ import annotations

import json


def _payload(html):
    return json.loads(
        html.split('type="application/json">')[1].split('</script>')[0])
