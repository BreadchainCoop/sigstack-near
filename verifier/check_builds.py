"""Checks the verifier's published builds, so a bad entry fails CI instead of the page.

Always:
- every builds/<hash>.json hashes to <hash>;
- every builds.json entry points at an existing file whose hash is its compose_hash;
- every app_compose pins its images by digest.

With --git (needs the full history, e.g. actions/checkout with fetch-depth: 0):
- each attested compose file is byte-for-byte docker/phala-demo.yaml at the entry's
  compose_commit;
- images_commit is that commit or one of its ancestors.

With --registry (needs network):
- for each of this repo's images, the tag sha-<images_commit> on ghcr.io resolves to the
  pinned digest, i.e. the images workflow pushed that digest when it built images_commit;
- for entries with "provenance": "github", `gh attestation verify` accepts each of this
  repo's images and its signed provenance names images_commit (needs gh and GH_TOKEN).

Usage: python3 verifier/check_builds.py [--git] [--registry]
"""

import hashlib
import json
import pathlib
import re
import subprocess
import sys
import urllib.error
import urllib.request

REPO = "BreadchainCoop/sigstack-near"
OWN_IMAGES = "ghcr.io/breadchaincoop/sigstack-near-"
COMPOSE_PATH = "docker/phala-demo.yaml"
ACCEPT = ",".join(
    [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    ]
)

root = pathlib.Path(__file__).resolve().parent
errors = []
notes = []


def git(*args):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True)


def check_git(entry, compose):
    commit = entry["compose_commit"]
    shown = git("show", f"{commit}:{COMPOSE_PATH}")
    if shown.returncode != 0:
        errors.append(f"{entry['compose_hash']}: no {COMPOSE_PATH} at {commit}")
    elif shown.stdout.decode() != compose:
        errors.append(f"{entry['compose_hash']}: attested compose isn't {COMPOSE_PATH} at {commit}")
    if git("merge-base", "--is-ancestor", entry["images_commit"], commit).returncode != 0:
        errors.append(f"{entry['compose_hash']}: images_commit isn't in {commit}'s history")


def tag_digest(name, tag):
    """The digest ghcr.io serves for name:tag, using an anonymous pull token."""
    with urllib.request.urlopen(f"https://ghcr.io/token?scope=repository:{name}:pull", timeout=30) as res:
        token = json.load(res)["token"]
    req = urllib.request.Request(
        f"https://ghcr.io/v2/{name}/manifests/{tag}",
        method="HEAD",
        headers={"Authorization": f"Bearer {token}", "Accept": ACCEPT},
    )
    with urllib.request.urlopen(req, timeout=30) as res:
        return res.headers["Docker-Content-Digest"]


def verified_commit(image):
    """The source commit in the image's GitHub provenance, after gh checks its signature."""
    out = subprocess.run(
        ["gh", "attestation", "verify", f"oci://{image}", "--repo", REPO, "--format", "json"],
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        raise RuntimeError(out.stderr.strip().splitlines()[-1] if out.stderr.strip() else "verify failed")
    commits = set()
    for result in json.loads(out.stdout):
        statement = result["verificationResult"]["statement"]
        for dep in statement["predicate"]["buildDefinition"]["resolvedDependencies"]:
            if "gitCommit" in dep.get("digest", {}):
                commits.add(dep["digest"]["gitCommit"])
    return commits


def check_registry(entry, images):
    for image in images:
        if not image.startswith(OWN_IMAGES):
            notes.append(f"{entry['compose_hash'][:8]}: {image.split('@')[0]} is third-party, pinned only")
            continue
        name, digest = image.split("@", 1)
        tag = "sha-" + entry["images_commit"]
        try:
            served = tag_digest(name.removeprefix("ghcr.io/"), tag)
        except (urllib.error.URLError, OSError, KeyError, ValueError) as e:
            errors.append(f"{entry['compose_hash']}: couldn't resolve {name}:{tag}: {e}")
            continue
        if served != digest:
            errors.append(f"{entry['compose_hash']}: {name}:{tag} is {served}, not the pinned {digest}")
        if entry.get("provenance") == "github":
            try:
                commits = verified_commit(image)
            except (RuntimeError, OSError, KeyError, ValueError) as e:
                errors.append(f"{entry['compose_hash']}: provenance for {image} didn't verify: {e}")
                continue
            if commits != {entry["images_commit"]}:
                errors.append(f"{entry['compose_hash']}: {image} provenance names {sorted(commits)}")


def main(argv):
    unknown = set(argv) - {"--git", "--registry"}
    if unknown:
        sys.exit(f"unknown arguments: {' '.join(sorted(unknown))}")

    for path in sorted((root / "builds").glob("*.json")):
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != path.stem:
            errors.append(f"{path.name}: sha256 is {actual}")

    index = json.loads((root / "builds.json").read_text())
    for entry in index:
        missing = [k for k in ("images_commit", "images_run", "compose_commit", "pinned_in_pr", "summary") if k not in entry]
        if missing:
            errors.append(f"{entry['compose_hash']}: missing {', '.join(missing)}")
            continue
        path = root / entry["app_compose"]
        if not path.is_file():
            errors.append(f"{entry['compose_hash']}: {entry['app_compose']} is missing")
            continue
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != entry["compose_hash"]:
            errors.append(f"{entry['app_compose']}: doesn't hash to {entry['compose_hash']}")
        compose = json.loads(data)["docker_compose_file"]
        images = re.findall(r"^\s*image:\s*[\"']?([^\s\"']+)", compose, re.M)
        for image in images:
            if "@sha256:" not in image:
                errors.append(f"{entry['app_compose']}: image {image} isn't pinned by digest")
        if "--git" in argv:
            check_git(entry, compose)
        if "--registry" in argv:
            check_registry(entry, [i for i in images if "@sha256:" in i])

    print("\n".join(notes))
    if errors:
        print("\n".join(errors))
        sys.exit(1)
    print(f"{len(index)} builds OK")


if __name__ == "__main__":
    main(sys.argv[1:])
