#!/usr/bin/env python3
"""Freeze the effective Freezer sources and dependency graph, or verify a copy."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def git(repo, *args):
    return subprocess.check_output(['git', *args], cwd=repo)


def file_record(path):
    if path.is_symlink():
        return {'link': os.readlink(path)}
    if path.is_file():
        return {'sha256': sha256(path), 'mode': path.stat().st_mode & 0o777}
    return None


def capture(repo, destination, evidence, prefix, repositories):
    """Copy Git's tracked and nonignored untracked inputs, including submodules."""
    head = git(repo, 'rev-parse', 'HEAD').decode().strip()
    patch = git(repo, 'diff', '--binary', 'HEAD')
    status = git(repo, 'status', '--porcelain=v1', '--untracked-files=all').decode()
    patch_path = evidence / (prefix.replace('/', '_') + '.patch')
    patch_path.write_bytes(patch)
    repositories[prefix] = {'head': head, 'status': status,
                            'patch': patch_path.name, 'patch_sha256': sha256(patch_path)}
    entries = git(repo, 'ls-files', '--stage', '-z').split(b'\0')
    paths = set()
    for entry in filter(None, entries):
        metadata, name = entry.split(b'\t', 1)
        mode, revision, stage = metadata.split()
        if stage != b'0':
            raise ValueError(f'unmerged input: {repo / os.fsdecode(name)}')
        relative = Path(os.fsdecode(name))
        if mode == b'160000':
            child = repo / relative
            child_prefix = f'{prefix}/{relative.as_posix()}'
            if (child / '.git').exists():
                capture(child, destination / relative, evidence, child_prefix, repositories)
            else:
                repositories[child_prefix] = {'gitlink': revision.decode(), 'initialized': False}
        else:
            paths.add(relative)
    paths.update(Path(os.fsdecode(name)) for name in
                 git(repo, 'ls-files', '--others', '--exclude-standard', '-z').split(b'\0') if name)
    copied = {}
    for relative in sorted(paths):
        source = repo / relative
        if not source.exists() and not source.is_symlink():
            continue  # A tracked deletion is preserved in the patch and file manifest.
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target, follow_symlinks=False)
        copied[relative] = file_record(target)
    if (git(repo, 'rev-parse', 'HEAD').decode().strip() != head
            or git(repo, 'diff', '--binary', 'HEAD') != patch
            or git(repo, 'status', '--porcelain=v1', '--untracked-files=all').decode() != status
            or any(file_record(repo / path) != record for path, record in copied.items())):
        raise ValueError(f'input changed while capturing {repo}; freeze again in a new directory')


def file_manifest(root):
    files = {}
    for path in sorted(root.rglob('*')):
        relative = path.relative_to(root).as_posix()
        record = file_record(path)
        if record is not None:
            files[relative] = record
    return files


def freeze(args):
    output = args.output.resolve()
    ckb = Path(__file__).resolve().parents[2]
    binding = args.binding.resolve()
    if output.is_relative_to(ckb) or output.is_relative_to(binding):
        raise ValueError('output must be outside both source repositories')
    output.mkdir(parents=True)
    sources = output / 'sources'
    patches = output / 'patches'
    patches.mkdir()
    repositories = {}
    for name, root in [('ckb', ckb), ('ckb-rocksdb', binding)]:
        capture(root, sources / name, patches, name, repositories)
    write_json(output / 'input-files.json', file_manifest(sources))
    write_json(output / 'repositories.json', repositories)

    # The override and its resolved lock are part of the candidate, never the caller's checkout.
    manifest = sources / 'ckb/Cargo.toml'
    with manifest.open('a') as stream:
        stream.write('\n[patch."https://github.com/nervosnetwork/ckb-rocksdb.git"]\n'
                     'ckb-rocksdb = { path = "../ckb-rocksdb" }\n')
    if args.lockfile:
        shutil.copyfile(args.lockfile, sources / 'ckb/Cargo.lock')
    command = ['cargo', 'metadata', '--format-version=1']
    if args.lockfile:
        command.append('--locked')
    if args.offline:
        command.append('--offline')
    with (output / 'cargo-metadata.json').open('w') as graph, (output / 'resolve.log').open('w') as log:
        subprocess.run(command, cwd=sources / 'ckb', stdout=graph, stderr=log, check=True)

    files = file_manifest(sources)
    for relative, record in files.items():
        if 'link' in record and not (sources / relative).resolve().is_relative_to(sources):
            raise ValueError(f'candidate symlink points outside the source bundle: {relative}')
    write_json(output / 'source-files.json', files)
    identity = sha256(output / 'source-files.json')
    with tarfile.open(output / 'sources.tar.xz', 'w:xz', preset=9) as archive:
        for relative in files:
            archive.add(sources / relative, arcname=relative, recursive=False)
    environment = {key: os.environ[key] for key in [
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_PROFILE_DEV_DEBUG',
        'CARGO_PROFILE_DEV_STRIP', 'CARGO_BUILD_JOBS',
        'CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'LIBCLANG_PATH', 'MACOSX_DEPLOYMENT_TARGET',
    ] if key in os.environ}
    write_json(output / 'candidate.json', {
        'source_id': identity, 'source_archive_sha256': sha256(output / 'sources.tar.xz'),
        'dependency_graph_sha256': sha256(output / 'cargo-metadata.json'),
        'platform': platform.platform(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
        'cargo': subprocess.check_output(['cargo', '-V'], text=True).strip(),
        'cc': subprocess.check_output([os.environ.get('CC', 'cc'), '--version'], text=True),
        'cxx': subprocess.check_output([os.environ.get('CXX', 'c++'), '--version'], text=True),
        'environment': environment, 'resolution_command': command,
        'dependency_override': 'ckb-rocksdb from the sibling sources/ckb-rocksdb directory',
        'boundary': 'Source/dependency reproduction; not bit-identical binaries or an offline vendor bundle. '
                    'Uninitialized optional submodules are identified in repositories.json.',
    })
    print(f'Frozen candidate {identity}\n{output}', flush=True)


def verify(args):
    candidate = args.candidate.resolve()
    expected = json.loads((candidate / 'source-files.json').read_text())
    actual = file_manifest(args.sources.resolve())
    changed = sorted(path for path in expected.keys() | actual.keys()
                     if expected.get(path) != actual.get(path))
    if changed:
        raise ValueError('source mismatch: ' + ', '.join(changed[:20]))
    record = json.loads((candidate / 'candidate.json').read_text())
    if sha256(candidate / 'source-files.json') != record['source_id']:
        raise ValueError('source manifest does not match candidate identity')
    if sha256(candidate / 'sources.tar.xz') != record['source_archive_sha256']:
        raise ValueError('source archive checksum mismatch')
    if sha256(candidate / 'cargo-metadata.json') != record['dependency_graph_sha256']:
        raise ValueError('dependency graph checksum mismatch')
    print(f'Verified {len(expected)} files for {record["source_id"]}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    capture_parser = commands.add_parser('freeze')
    capture_parser.add_argument('--binding', type=Path, required=True)
    capture_parser.add_argument('--lockfile', type=Path, help='Reuse an already resolved candidate lock')
    capture_parser.add_argument('--offline', action='store_true')
    capture_parser.add_argument('output', type=Path, help='New directory outside both repositories')
    capture_parser.set_defaults(action=freeze)
    verify_parser = commands.add_parser('verify')
    verify_parser.add_argument('candidate', type=Path)
    verify_parser.add_argument('sources', type=Path, help='Extracted source root, without build outputs')
    verify_parser.set_defaults(action=verify)
    args = parser.parse_args()
    args.action(args)


if __name__ == '__main__':
    main()
