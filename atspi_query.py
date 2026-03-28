#!/usr/bin/env python3
"""
Query the AT-SPI accessibility tree for a given PID.
Outputs JSON: list of {text, x, y, w, h} in screen coordinates.
Usage: python3 atspi_query.py <pid>
"""
import gi, sys, json

gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

# Roles that carry meaningful text content
TEXT_ROLES = {
    Atspi.Role.LABEL, Atspi.Role.PUSH_BUTTON, Atspi.Role.MENU_ITEM,
    Atspi.Role.ENTRY, Atspi.Role.TEXT, Atspi.Role.HEADING, Atspi.Role.LINK,
    Atspi.Role.TOGGLE_BUTTON, Atspi.Role.CHECK_BOX, Atspi.Role.RADIO_BUTTON,
    Atspi.Role.COLUMN_HEADER, Atspi.Role.ROW_HEADER, Atspi.Role.MENU,
    Atspi.Role.MENU_BAR, Atspi.Role.TAB, Atspi.Role.COMBO_BOX,
}

def collect(obj, results, depth=0):
    if depth > 40:
        return
    try:
        role = obj.get_role()
        name = (obj.get_name() or '').strip()

        # Prefer the text interface content over the accessible name
        text_content = ''
        try:
            ti = obj.get_text_iface()
            if ti:
                text_content = (ti.get_text(0, -1) or '').strip()
        except Exception:
            pass

        display = text_content or name

        if display and role in TEXT_ROLES:
            try:
                ext = obj.get_extents(Atspi.CoordType.SCREEN)
                if ext.width > 0 and ext.height > 0:
                    results.append({
                        'text': display,
                        'x': ext.x,
                        'y': ext.y,
                        'w': ext.width,
                        'h': ext.height,
                    })
            except Exception:
                pass

        for i in range(obj.get_child_count()):
            try:
                child = obj.get_child_at_index(i)
                if child:
                    collect(child, results, depth + 1)
            except Exception:
                pass
    except Exception:
        pass

if __name__ == '__main__':
    Atspi.init()
    pid = int(sys.argv[1])
    desktop = Atspi.get_desktop(0)
    results = []
    for i in range(desktop.get_child_count()):
        try:
            app = desktop.get_child_at_index(i)
            if app and app.get_process_id() == pid:
                collect(app, results)
                break
        except Exception:
            pass
    print(json.dumps(results))
