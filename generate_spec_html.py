#!/usr/bin/env python3
"""Generate HCDF spec browser HTML from hcdf.xsd.

Parses the XSD schema and produces a single-file static HTML page with
expandable element trees, modeled after sdformat.org's specification browser.

Usage:
    python3 generate_spec_html.py [hcdf.xsd] [output.html]
"""
import sys
from lxml import etree
from collections import OrderedDict

XS = "http://www.w3.org/2001/XMLSchema"

# Tab definitions: (tab_id, display_name, root_type_name)
TABS = [
    ("hcdf",        "HCDF",         None),
    ("comp",        "Comp",         "comp"),
    ("joint",       "Joint",        "joint"),
    ("group",       "Group",        "joint_group"),
    ("state",       "State",        "kinematic_state"),
    ("switch",      "Switch",       "switch"),
    ("sensor",      "Sensor",       "sensor"),
    ("motor",       "Motor",        "motor"),
    ("hmi",         "HMI",          "hmi_element"),
    ("surface",     "Surface",      "surface_element"),
    ("power",       "Power",        "power_source"),
    ("port",        "Port",         "port"),
    ("antenna",     "Antenna",      "antenna"),
    ("network",     "Network",      "network"),
    ("link",        "Link",         "link"),
    ("bus",         "Bus",          "bus"),
    ("chain",       "Chain",        "chain"),
    ("mesh",        "Mesh",         "wireless_mesh"),
    ("transmission","Transmission", "transmission"),
    ("material",    "Material",     "material_global"),
    ("geometry",    "Geometry",     "geometry"),
]

STREAM_TABS = [
    ("stream-profile", "Stream Profile", None),   # root element
    ("stream",         "Stream",         "stream"),
    ("stream-group",   "Stream Group",   "stream_group"),
    ("frer",           "FRER",           "frer"),
]


class XsdParser:
    """Parse an XSD file and extract type/element information."""

    def __init__(self, xsd_path):
        self.tree = etree.parse(xsd_path)
        self.root = self.tree.getroot()
        self.types = {}      # name -> element
        self.enums = {}      # name -> [values]
        self._parse_types()

    def _parse_types(self):
        for t in self.root.findall(f"{{{XS}}}complexType"):
            name = t.get("name")
            if name:
                self.types[name] = t
        for t in self.root.findall(f"{{{XS}}}simpleType"):
            name = t.get("name")
            if name:
                restriction = t.find(f"{{{XS}}}restriction")
                if restriction is not None:
                    vals = [e.get("value") for e in restriction.findall(f"{{{XS}}}enumeration")]
                    if vals:
                        self.enums[name] = vals

    def get_root_element(self):
        return self.root.find(f"{{{XS}}}element")

    def get_annotation(self, elem):
        ann = elem.find(f"{{{XS}}}annotation")
        if ann is not None:
            doc = ann.find(f"{{{XS}}}documentation")
            if doc is not None and doc.text:
                return doc.text.strip()
        return None

    def get_type_children(self, type_name):
        """Get all child elements and attributes for a named complex type.
        Follows xs:extension to include base type children."""
        t = self.types.get(type_name)
        if t is None:
            return [], []

        elements = []
        attributes = []

        # Check for xs:complexContent/xs:extension (inheritance)
        cc = t.find(f"{{{XS}}}complexContent")
        if cc is not None:
            ext = cc.find(f"{{{XS}}}extension")
            if ext is not None:
                base_name = ext.get("base")
                if base_name:
                    # Recursively get base type children first
                    base_elems, base_attrs = self.get_type_children(base_name)
                    elements.extend(base_elems)
                    attributes.extend(base_attrs)
                # Then get extension's own attributes
                for attr in ext.findall(f"{{{XS}}}attribute"):
                    attributes.append(self._parse_attribute(attr))
                # And extension's own elements
                for container_tag in ["sequence", "all", "choice"]:
                    container = ext.find(f"{{{XS}}}{container_tag}")
                    if container is not None:
                        for child in container.findall(f"{{{XS}}}element"):
                            elements.append(self._parse_element(child, container_tag))
                return elements, attributes

        # Collect attributes directly on the type
        for attr in t.findall(f"{{{XS}}}attribute"):
            attributes.append(self._parse_attribute(attr))

        # Collect elements from sequence, all, or choice
        for container_tag in ["sequence", "all", "choice"]:
            container = t.find(f"{{{XS}}}{container_tag}")
            if container is not None:
                compositor = container_tag
                for child in container.findall(f"{{{XS}}}element"):
                    elements.append(self._parse_element(child, compositor))
                # Also check for nested choice inside sequence
                if container_tag == "sequence":
                    for inner_choice in container.findall(f"{{{XS}}}choice"):
                        for child in inner_choice.findall(f"{{{XS}}}element"):
                            elements.append(self._parse_element(child, "choice"))

        return elements, attributes

    def _parse_attribute(self, attr):
        name = attr.get("name", "?")
        type_name = attr.get("type", "xs:string")
        required = attr.get("use") == "required"
        default = attr.get("default")
        enum_values = self.enums.get(type_name, [])
        doc = self.get_annotation(attr)
        return {
            "name": name,
            "type": type_name,
            "required": required,
            "default": default,
            "enum": enum_values,
            "doc": doc,
        }

    def _parse_element(self, elem, compositor="sequence"):
        name = elem.get("name", "?")
        type_name = elem.get("type", "")
        min_occurs = elem.get("minOccurs", "1")
        max_occurs = elem.get("maxOccurs", "1")
        required = min_occurs != "0" and compositor != "choice"
        multiple = max_occurs == "unbounded"
        doc = self.get_annotation(elem)

        # Check if type is a known complex type (has children)
        has_children = type_name in self.types
        is_enum = type_name in self.enums
        enum_values = self.enums.get(type_name, [])

        # Determine which tab this type belongs to (for cross-references)
        # tab_ref is resolved later by HtmlGenerator using its active tab list
        tab_ref = None

        return {
            "name": name,
            "type": type_name,
            "required": required,
            "multiple": multiple,
            "has_children": has_children,
            "is_enum": is_enum,
            "enum": enum_values,
            "tab_ref": tab_ref,
            "doc": doc,
        }


class HtmlGenerator:
    """Generate the spec browser HTML."""

    def __init__(self, parser, title="HCDF 1.0", tabs=None):
        self.parser = parser
        self.depth_limit = 6
        self.title = title
        self.tabs = tabs if tabs is not None else TABS

    def generate(self):
        first_tab_id = self.tabs[0][0]
        parts = [self._header()]
        parts.append(self._tabs_html())
        parts.append('<div class="tab-content">')
        for tab_id, tab_name, type_name in self.tabs:
            active = "active" if tab_id == first_tab_id else ""
            parts.append(f'<div class="tab-pane {active}" id="tab-{tab_id}">')
            parts.append('<div class="tree"><ul>')
            if type_name is None:
                parts.append(self._render_root())
            elif type_name:
                parts.append(self._render_type(type_name, 0))
            parts.append('</ul></div></div>')
        parts.append('</div>')
        parts.append(self._footer())
        return "\n".join(parts)

    def _render_root(self):
        """Render the root <hcdf> element."""
        root_el = self.parser.get_root_element()
        if root_el is None:
            return "<li>No root element found</li>"

        lines = []
        # Get the anonymous complex type inside the root element
        ct = root_el.find(f"{{{XS}}}complexType")
        if ct is None:
            return "<li>No complex type</li>"

        # Root attributes
        for attr in ct.findall(f"{{{XS}}}attribute"):
            a = self.parser._parse_attribute(attr)
            lines.append(self._attr_li(a))

        # Root children
        for container_tag in ["sequence", "all", "choice"]:
            container = ct.find(f"{{{XS}}}{container_tag}")
            if container is not None:
                for child in container.findall(f"{{{XS}}}element"):
                    e = self.parser._parse_element(child, container_tag)
                    lines.append(self._element_li(e, 0))

        return "\n".join(lines)

    def _render_type(self, type_name, depth):
        """Render all children of a named type."""
        if depth > self.depth_limit:
            return ""
        elements, attributes = self.parser.get_type_children(type_name)
        lines = []
        for a in attributes:
            lines.append(self._attr_li(a))
        for e in elements:
            lines.append(self._element_li(e, depth))
        return "\n".join(lines)

    def _element_li(self, e, depth):
        """Render a single element as an <li>."""
        name = e["name"]
        has_children = e["has_children"] and depth < self.depth_limit
        # Resolve tab cross-reference from the active tab list
        tab_ref = None
        for tab_id, _, tab_type in self.tabs:
            if tab_type == e["type"]:
                tab_ref = tab_id
                break

        # Icon
        if tab_ref and tab_ref != self._current_tab_hack(e):
            icon = '<span class="tree-collapse glyphicon glyphicon-chevron-right"></span>'
        elif has_children:
            icon = '<span class="tree-collapse glyphicon glyphicon-plus"></span>'
        else:
            icon = '<span class="tree-collapse glyphicon glyphicon-minus"></span>'

        # Required badge
        req = ' <small class="req">(required)</small>' if e["required"] else ""
        mult = ' <small class="mult">[0..*]</small>' if e["multiple"] else ""

        # Type info
        type_str = ""
        if e["is_enum"]:
            vals = " | ".join(e["enum"])
            type_str = f'<div class="type-info"><b>Values:</b> {vals}</div>'
        elif e["type"] and e["type"] not in self.parser.types:
            type_str = f'<div class="type-info"><b>Type:</b> {_esc(e["type"])}</div>'

        # Description
        doc = ""
        if e.get("doc"):
            doc = f'<div class="desc">{_esc(e["doc"])}</div>'

        # Children
        children_html = ""
        if has_children and not tab_ref:
            inner = self._render_type(e["type"], depth + 1)
            if inner:
                children_html = f"<ul>{inner}</ul>"

        # Cross-tab link
        tab_link = ""
        elem_extra = ""
        if tab_ref:
            tab_link = f' onclick="switchTab(\'{tab_ref}\')"'
            icon = f'<span class="tree-collapse glyphicon glyphicon-chevron-right" style="cursor:pointer"{tab_link}></span>'
            elem_extra = f' style="cursor:pointer"{tab_link}'

        return (
            f'<li>'
            f'{icon}'
            f'<span class="tree-element"{elem_extra}>'
            f'<h5>&lt;{_esc(name)}&gt; <small>Element</small></h5>'
            f'{req}{mult}{type_str}{doc}'
            f'</span>'
            f'{children_html}'
            f'</li>'
        )

    def _attr_li(self, a):
        """Render a single attribute as an <li>."""
        name = a["name"]
        req = " (required)" if a["required"] else ""
        default = f', default="{_esc(a["default"])}"' if a["default"] else ""

        type_str = _esc(a["type"])
        if a["enum"]:
            type_str = " | ".join(a["enum"])

        doc = ""
        if a.get("doc"):
            doc = f' &mdash; {_esc(a["doc"])}'

        return (
            f'<li>'
            f'<span class="tree-attribute">'
            f'<h5>{_esc(name)} <small>Attribute</small></h5>'
            f'<div class="type-info"><b>Type:</b> {type_str}{req}{default}{doc}</div>'
            f'</span>'
            f'</li>'
        )

    def _current_tab_hack(self, e):
        """Determine which tab an element belongs to (for avoiding self-refs)."""
        return None

    def _tabs_html(self):
        lines = ['<ul class="nav nav-tabs" role="tablist" id="spec-tabs">']
        for i, (tab_id, tab_name, _) in enumerate(self.tabs):
            active = ' class="active"' if i == 0 else ""
            lines.append(
                f'<li{active}><a href="#tab-{tab_id}" data-toggle="tab">{tab_name}</a></li>'
            )
        lines.append("</ul>")
        return "\n".join(lines)

    def _header(self):
        title = _esc(self.title)
        return f'''<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} Specification Browser</title>
<link rel="stylesheet" href="https://maxcdn.bootstrapcdn.com/bootstrap/3.3.7/css/bootstrap.min.css">
<link rel="stylesheet" href="/assets/style.css"/>
<style>
/* Spec browser overrides for dark theme */
.spec-container {{ max-width:1200px; margin:0 auto; padding:20px; }}
.tree {{ background-color:#161b22; border-radius:6px; min-height:20px; padding:19px; overflow-y:auto; border:1px solid #30363d; }}
.tree ul {{ margin:0; padding-left:25px; }}
.tree li {{ list-style-type:none; margin:0 !important; margin-bottom:0 !important; padding:2px 0 0 2px; position:relative; }}
.tree li::before, .tree li::after {{ display:none; }}
.tree>ul>li::before, .tree>ul>li::after {{ display:none; }}
.tree li.parent_li > span {{ cursor:pointer; transition:background-color 0.15s; }}
.tree li.parent_li > span.tree-element:hover {{ background-color:#244a6f; border-color:#58a6ff; }}
.tree li.parent_li > span.tree-attribute:hover {{ background-color:#2a5a3a; border-color:#3fb950; }}
.tree-element {{ word-wrap:break-word; border:1px solid #2d5a8a; border-radius:5px; display:inline-block;
  line-height:14px; padding:4px 8px; background-color:#1a3a5c; color:#79c0ff; margin:2px 0; }}
.tree-attribute {{ word-wrap:break-word; border:1px solid #2a6a3a; border-radius:5px; display:inline-block;
  line-height:14px; padding:4px 8px; background-color:#1a3a2a; color:#56d364; margin:2px 0; }}
.tree-element h5, .tree-attribute h5 {{ display:inline; margin:0; padding:0; font-size:13px; color:inherit; font-weight:600;
  background:none; border:none; line-height:inherit; }}
.tree-element h5 small, .tree-attribute h5 small {{ color:#8b949e; font-size:10px; font-weight:normal; }}
.tree-element div, .tree-attribute div {{ margin-top:2px; }}
.type-info {{ font-size:11px; color:#8b949e; padding:2px 4px; }}
.desc {{ font-size:11px; color:#8b949e; padding:2px 4px; font-style:italic; }}
.req {{ color:#f85149; font-weight:bold; }}
.mult {{ color:#8b949e; }}
.tree-collapse {{ margin-right:4px; cursor:pointer; font-size:10px; color:#8b949e; background:none !important;
  border:none !important; box-shadow:none !important; padding:0 !important; }}
h1 {{ color:#e6edf3; }}
h1 small {{ font-size:14px; color:#8b949e; }}
.key-box {{ background:#161b22; padding:12px; border-radius:6px; margin-bottom:15px; border:1px solid #30363d; color:#e6edf3; }}
.key-box span {{ margin-right:12px; }}
.nav-tabs {{ border-bottom-color:#30363d !important; }}
.nav-tabs > li > a {{ color:#8b949e !important; background:transparent !important; border-color:transparent !important; }}
.nav-tabs > li > a:hover {{ color:#e6edf3 !important; background:#161b22 !important; border-color:#30363d #30363d transparent !important; }}
.nav-tabs > li.active > a, .nav-tabs > li.active > a:focus, .nav-tabs > li.active > a:hover {{
  color:#58a6ff !important; background:#161b22 !important; border-color:#30363d #30363d #161b22 !important; }}
.tab-content {{ padding-top:15px; }}
@media (max-width:768px) {{ .nav-tabs > li > a {{ font-size:10px; padding:6px 6px; }} }}
</style>
</head>
<body>

<nav>
  <a href="/" class="brand">HCDF</a>
  <button class="hamburger" aria-label="Menu">
    <span></span><span></span><span></span>
  </button>
  <div class="nav-links">
    <div class="dropdown">
      <a>Specification &#9662;</a>
      <div class="dropdown-content">
        <a href="/spec/">Overview</a>
        <a href="/spec/core.html">Core Schema</a>
        <a href="/spec/stream-profile.html">Stream Profile</a>
        <a href="/spec/extensions/ros2.html">ROS 2 Extension</a>
        <a href="/spec/extensions/gazebo.html">Gazebo Extension</a>
        <a href="/spec/extensions/1722.html">IEEE 1722 Extension</a>
      </div>
    </div>
    <div class="dropdown">
      <a>Examples &#9662;</a>
      <div class="dropdown-content">
        <a href="/examples/">All Examples</a>
        <a href="/examples/#humanoid">Humanoid Mobile Base</a>
        <a href="/examples/#drone">Drone Quadrotor</a>
      </div>
    </div>
    <a href="/design/">Design</a>
    <div class="dropdown">
      <a>Extensions &#9662;</a>
      <div class="dropdown-content">
        <a href="/extensions/">Guide</a>
        <a href="/extensions/imu-stability.html">IMU Stability</a>
      </div>
    </div>
    <a href="/tools/">Tools</a>
    <a href="/about/">About</a>
    <a href="https://github.com/CogniPilot/hcdformat">GitHub</a>
  </div>
</nav>

<div class="spec-container">
<h1>{title} <small>Specification Browser</small></h1>
<div class="key-box">
  <span class="tree-collapse glyphicon glyphicon-stop" style="color:#1a3a5c"></span> Element
  <span class="tree-collapse glyphicon glyphicon-stop" style="color:#1a3a2a"></span> Attribute
  <span class="tree-collapse glyphicon glyphicon-plus"></span> Has children (click to expand)
  <span class="tree-collapse glyphicon glyphicon-minus"></span> Leaf node
  <span class="tree-collapse glyphicon glyphicon-chevron-right"></span> Defined in another tab (click to switch)
</div>
'''

    def _footer(self):
        return '''
</div>

<footer>
  <p>HCDF is a project of the <a href="https://cognipilot.org">CogniPilot Foundation</a>. Licensed under <a href="https://www.apache.org/licenses/LICENSE-2.0">Apache 2.0</a>.</p>
</footer>

<script src="https://code.jquery.com/jquery-1.12.4.min.js"></script>
<script src="https://maxcdn.bootstrapcdn.com/bootstrap/3.3.7/js/bootstrap.min.js"></script>
<script src="/assets/nav.js"></script>
<script>
function switchTab(tabId) {
    $('#spec-tabs a[href="#tab-' + tabId + '"]').tab('show');
}
$(function () {
    // Mark parent nodes
    $('.tree li:has(ul)').addClass('parent_li')
        .find(' > span.tree-element, > span.tree-attribute').attr('title', 'Collapse this branch');
    // Start collapsed
    $('.tree li.parent_li > ul > li').hide();
    // Toggle on click (only on tree-element/tree-attribute spans, not chevrons)
    $('.tree li.parent_li > span.tree-element, .tree li.parent_li > span.tree-attribute').on('click', function (e) {
        var children = $(this).parent('li.parent_li').find(' > ul > li');
        if (children.is(":visible")) {
            children.hide('fast');
        } else {
            children.show('fast');
        }
        e.stopPropagation();
    });
    // Cross-tab chevron clicks
    $('.tree .glyphicon-chevron-right[onclick]').on('click', function(e) {
        e.stopPropagation();
    });
});
</script>
</body>
</html>'''


def _esc(s):
    """HTML-escape a string."""
    if s is None:
        return ""
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def _auto_discover_tabs(parser):
    """Build tabs automatically from root element and named complex types.

    Used for extension XSDs and any schema that doesn't match the
    predefined TABS or STREAM_TABS layouts.

    When the root element references a named type (e.g. type="topic_map"),
    the root tab renders that named type directly instead of creating a
    separate empty tab plus a duplicate tab for the named type.
    """
    tabs = []
    root_type_ref = None  # named type referenced by root element, if any
    root_el = parser.get_root_element()
    if root_el is not None:
        root_name = root_el.get("name", "root")
        display = root_name.replace("-", " ").replace("_", " ").title()
        root_type_ref = root_el.get("type")  # e.g. "topic_map" or None
        if root_type_ref and root_type_ref in parser.types:
            # Root references a named type — render that type directly
            tabs.append((root_name, display, root_type_ref))
        else:
            # Root has an inline anonymous complex type — use _render_root
            tabs.append((root_name, display, None))
    # Add a tab for each named complex type (skip the one already used by root)
    for type_name in parser.types:
        if type_name == root_type_ref:
            continue
        display = type_name.replace("_", " ").replace("-", " ").title()
        tabs.append((type_name, display, type_name))
    return tabs


def _detect_schema_kind(xsd_path):
    """Determine title and tabs based on the XSD filename."""
    path_lower = xsd_path.lower()
    if 'stream' in path_lower and 'ext' not in path_lower:
        return "HCDF Stream Profile 1.0", STREAM_TABS
    if 'ext' not in path_lower and 'extension' not in path_lower:
        return "HCDF 1.0", TABS
    # Extension XSD — derive title from filename
    # e.g., "hcdf-ext-ros2.xsd" -> "HCDF ROS 2 Extension"
    import os
    base = os.path.splitext(os.path.basename(xsd_path))[0]  # hcdf-ext-ros2
    # Strip "hcdf-ext-" prefix if present
    label = base
    for prefix in ("hcdf-ext-", "hcdf-extension-"):
        if label.lower().startswith(prefix):
            label = label[len(prefix):]
            break
    # Capitalise known tokens, title-case the rest
    known_upper = {"ros2": "ROS 2", "ros": "ROS", "gazebo": "Gazebo",
                   "isaac": "Isaac", "nav2": "Nav2", "moveit": "MoveIt",
                   "1722": "IEEE 1722"}
    words = label.replace("-", " ").replace("_", " ").split()
    titled = []
    for w in words:
        if w.lower() in known_upper:
            titled.append(known_upper[w.lower()])
        else:
            titled.append(w.title())
    title = "HCDF " + " ".join(titled) + " Extension"
    return title, None  # None signals auto-discover


def main():
    xsd_path = sys.argv[1] if len(sys.argv) > 1 else "hcdf.xsd"
    out_path = sys.argv[2] if len(sys.argv) > 2 else "spec.html"

    title, tabs = _detect_schema_kind(xsd_path)

    parser = XsdParser(xsd_path)

    # Auto-discover tabs for extension XSDs (or any schema without predefined tabs)
    if tabs is None:
        tabs = _auto_discover_tabs(parser)

    gen = HtmlGenerator(parser, title=title, tabs=tabs)
    html = gen.generate()

    with open(out_path, "w") as f:
        f.write(html)
    print(f"Generated {out_path} from {xsd_path}")
    print(f"  Types: {len(parser.types)}, Enums: {len(parser.enums)}")


if __name__ == "__main__":
    main()
