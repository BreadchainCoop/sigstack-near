"""Checks the verifier's published builds, so a bad entry fails CI instead of the page.

- every builds/<hash>.json hashes to <hash>;
- every builds.json entry points at an existing file whose hash is its compose_hash;
- every app_compose pins its images by digest.

Usage: python3 verifier/check_builds.py
"""

import hashlib
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent
errors = []

for path in sorted((root / "builds").glob("*.json")):
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != path.stem:
        errors.append(f"{path.name}: sha256 is {actual}")

index = json.loads((root / "builds.json").read_text())
for entry in index:
    path = root / entry["app_compose"]
    if not path.is_file():
        errors.append(f"{entry['compose_hash']}: {entry['app_compose']} is missing")
        continue
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != entry["compose_hash"]:
        errors.append(f"{entry['app_compose']}: doesn't hash to {entry['compose_hash']}")
    compose = json.loads(data)["docker_compose_file"]
    for image in re.findall(r"^\s*image:\s*[\"']?([^\s\"']+)", compose, re.M):
        if "@sha256:" not in image:
            errors.append(f"{entry['app_compose']}: image {image} isn't pinned by digest")
    for key in ("images_commit", "images_run", "pinned_in_pr", "summary"):
        if key not in entry:
            errors.append(f"{entry['compose_hash']}: missing {key}")

if errors:
    print("\n".join(errors))
    sys.exit(1)
print(f"{len(index)} builds OK")
