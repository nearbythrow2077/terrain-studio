"""Collect notices for the dependencies selected for this package's target."""

import json
from pathlib import Path
import sys

metadata = json.loads(Path(sys.argv[1]).read_bytes())
packages = {item["id"]: item for item in metadata["packages"]}
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
root = metadata["resolve"]["root"]
visited = {root}
pending = [root]

while pending:
    for dependency in nodes[pending.pop()]["deps"]:
        target = dependency["pkg"]
        if target not in visited and any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
            visited.add(target)
            pending.append(target)

dependencies = sorted(
    (packages[key] for key in visited if key != root),
    key=lambda package: (package["name"], package["version"]),
)
print("Terrain Studio third-party notices")
for package in dependencies:
    name, version = package["name"], package["version"]
    directory = Path(package["manifest_path"]).parent
    explicit = package["license_file"]
    notices = {
        path for path in directory.iterdir()
        if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "UNLICENSE", "OFL", "LICENCE"))
    }
    if explicit:
        notices.add(directory / explicit)
    if name == "epaint_default_fonts":
        notices.update(
            path for path in (directory / "fonts").glob("*")
            if path.is_file() and path.suffix.lower() not in {".otf", ".ttf"}
        )
    print(f"\n{name} {version} [{package['license'] or 'see license file'}]")
    if package["repository"]:
        print(package["repository"])
    for path in sorted(notices):
        if path.is_file():
            print(f"\n{path.relative_to(directory)}\n{path.read_text(errors='replace').rstrip()}")
