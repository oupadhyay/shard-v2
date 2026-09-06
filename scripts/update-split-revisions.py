#!/usr/bin/env python3
"""Prepare coordinated split pin updates from checked-out, merged snapshots."""

import argparse
import json
import re
import subprocess
import tomllib
from pathlib import Path

SPLITS = ("shard-tool-api", "shard-provider", "shard-external-tools")
OWNER = "https://github.com/oupadhyay/"


def run(*args, cwd):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def pin(manifest, name):
    dependency = tomllib.loads(manifest)["dependencies"][name]
    if dependency.get("git") != OWNER + name or not re.fullmatch(
        r"[0-9a-f]{40}", dependency.get("rev", "")
    ) or "path" in dependency or "branch" in dependency or "tag" in dependency:
        raise ValueError(f"{name}: expected a full immutable GitHub revision")
    return dependency["rev"]


def replace_pin(manifest, name, revision):
    old = pin(manifest, name)
    pattern = rf'(?m)^(\s*{re.escape(name)}\s*=\s*\{{[^\n]*\brev\s*=\s*"){old}("[^\n]*\}}\s*)$'
    result, count = re.subn(pattern, lambda m: m[1] + revision + m[2], manifest)
    if count != 1:
        raise ValueError(f"{name}: expected one inline dependency declaration")
    return result


def planned_pins(consumer, sources):
    heads = {name: run("git", "rev-parse", "HEAD", cwd=sources / name) for name in SPLITS}
    for sha in heads.values():
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise ValueError("Invalid source revision")
    if consumer != "shard-v2":
        return {"shard-tool-api": heads["shard-tool-api"]}
    api_pins = {
        pin((sources / name / "Cargo.toml").read_text(), "shard-tool-api")
        for name in SPLITS[1:]
    }
    if api_pins != {heads["shard-tool-api"]}:
        raise ValueError(
            "Waiting for both standalone tool-api update PRs to merge: "
            "provider, external-tools and latest tool-api main must agree. "
            "No host files were changed."
        )
    return heads


def audit_graph(metadata, desired):
    for name, revision in desired.items():
        packages = [p for p in metadata["packages"] if p["name"] == name]
        source = f"git+{OWNER}{name}?rev={revision}#{revision}"
        if len(packages) != 1 or packages[0]["source"] != source:
            raise ValueError(f"Resolved graph does not contain exactly one expected {name}")


def prepare(consumer, target, sources, body):
    desired = planned_pins(consumer, sources)
    crate = target / "src-tauri" if consumer == "shard-v2" else target
    manifest_path = crate / "Cargo.toml"
    original = manifest_path.read_text()
    manifest = original
    changes = []
    for name, revision in desired.items():
        old = pin(manifest, name)
        if old != revision:
            changes.append(f"- [{name} changes]({OWNER}{name}/compare/{old}...{revision})")
            manifest = replace_pin(manifest, name, revision)
    if not changes:
        print("Already current; no update needed.")
        return
    manifest_path.write_text(manifest)
    if consumer != "shard-v2":
        # Standalone boundary audits deliberately pin the expected contract source.
        audit = target / "scripts/audit_dependency_boundary.py"
        text, count = re.subn(
            r'(?m)^TOOL_API_REVISION = "[0-9a-f]{40}"$',
            f'TOOL_API_REVISION = "{desired["shard-tool-api"]}"',
            audit.read_text(),
        )
        if count != 1:
            raise ValueError("Expected exactly one tool-api revision in boundary audit")
        audit.write_text(text)
    # Resolve changed Git pins without broadly upgrading registry dependencies.
    metadata = json.loads(run("cargo", "metadata", "--format-version", "1", cwd=crate))
    audit_graph(metadata, desired)
    run("cargo", "metadata", "--locked", "--format-version", "1", cwd=crate)
    run("git", "diff", "--check", cwd=target)
    checklist = (
        "- [ ] Full host Rust checks and frontend tests/build pass.\n"
        "- [ ] Native GUI: streaming, cancellation, persistence, tool hooks/events/output.\n"
        "- [ ] External tools: web/open-URL, YouTube short/long summary, heartbeat restrictions.\n"
        "- [ ] Provider: Gemini Files upload/use/delete, embedding/search, vision fallback.\n"
        "- [ ] Record results and any platform/credential limitations before marking ready."
        if consumer == "shard-v2" else
        "- [ ] Standalone fmt/check/test/clippy/doc and dependency audit pass.\n"
        "- [ ] Review contract compatibility; merge before the host pin update."
    )
    body.write_text(
        "## Automated split revision update\n\n" + "\n".join(changes)
        + "\n\nPins come from merged main snapshots. Cargo.lock was regenerated and "
        "the resolved split sources checked. This does not certify runtime compatibility.\n\n"
        + checklist + "\n\nNo auto-merge. Automation owns this branch; do not edit it manually.\n"
    )
    print("Prepared revision update; review and GUI verification remain required.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("consumer", choices=("shard-v2", *SPLITS[1:]))
    parser.add_argument("target", type=Path)
    parser.add_argument("sources", type=Path)
    parser.add_argument("body", type=Path)
    args = parser.parse_args()
    prepare(args.consumer, args.target, args.sources, args.body)
