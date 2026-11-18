#!/usr/bin/env python3
"""Build published old engines and verify hot DB upgrade, archival and restore."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time
import tomllib


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='New directory for sources, logs and DBs')
    parser.add_argument('--candidate', type=Path, help='Frozen manifest for these extracted sources')
    parser.add_argument('--legacy-lockfiles', type=Path,
                        help='Reuse VERSION/Cargo.lock from an earlier recorded run')
    parser.add_argument('--legacy-target', type=Path, help='Separate reusable old-engine build cache')
    parser.add_argument('--offline', action='store_true', help='Require dependencies in the Cargo cache')
    args = parser.parse_args()
    output = args.output.resolve()
    repo = Path(__file__).resolve().parents[2]
    source_root = repo.parent if args.candidate else repo
    if output.is_relative_to(source_root):
        parser.error('output must be outside the source checkout')
    consumer_env = dict(os.environ)
    consumer_target = Path(consumer_env.get('CARGO_TARGET_DIR', str(output / 'consumer-target')))
    consumer_target = (repo / consumer_target).resolve()
    if args.candidate and consumer_target.is_relative_to(source_root):
        parser.error('consumer build target must be outside the frozen sources')
    consumer_env['CARGO_TARGET_DIR'] = str(consumer_target)
    output.mkdir(parents=True)
    metadata = {
        'platform': platform.platform(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
        'consumer_lock_sha256': digest(repo / 'Cargo.lock'),
        'environment': {name: consumer_env[name] for name in [
            'CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_PROFILE_DEV_DEBUG',
            'CARGO_PROFILE_DEV_STRIP', 'CARGO_BUILD_JOBS', 'RUSTFLAGS',
            'LIBCLANG_PATH', 'DYLD_FALLBACK_LIBRARY_PATH',
        ] if name in consumer_env},
        'checks': [],
        'runs': [],
    }

    def save_manifest():
        (output / 'manifest.json').write_text(json.dumps(metadata, indent=2) + '\n')

    def run(name, command, env, log, checks):
        print(name, flush=True)
        started = time.time()
        with log.open('w') as stream:
            result = subprocess.run(command, cwd=repo, env=env,
                                    stdout=stream, stderr=subprocess.STDOUT)
        record = {'name': name, 'command': command, 'cwd': str(repo),
                  'started_at': started, 'seconds': round(time.time() - started, 3),
                  'exit_code': result.returncode, 'log': str(log.relative_to(output)),
                  'log_sha256': digest(log)}
        checks.append(record)
        save_manifest()
        result.check_returncode()
        return record

    if args.candidate:
        candidate = args.candidate.resolve()
        metadata['consumer_candidate'] = json.loads((candidate / 'candidate.json').read_text())
        verify = [sys.executable, str(Path(__file__).with_name('freeze-candidate.py')),
                  'verify', str(candidate), str(repo.parent)]
        run('source-before', verify, os.environ, output / 'source-before.log', metadata['checks'])
    else:
        metadata['consumer_head'] = subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
        patch = output / 'consumer.patch'
        patch.write_bytes(subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=repo))
        metadata['consumer_patch_sha256'] = digest(patch)
        metadata['source_boundary'] = 'Git HEAD and tracked diff only; use --candidate for full inputs.'

    test = 'tests::upgrade::old_engine_hot_data_survives_upgrade_archival_and_backup'
    for version in ['0.21.1', '0.22.2']:
        project = output / version
        (project / 'src').mkdir(parents=True)
        shutil.copyfile(Path(__file__).with_name('legacy_writer.rs'), project / 'src/main.rs')
        (project / 'Cargo.toml').write_text(f'''[package]
name = "legacy-hot-db"
version = "{version}"
edition = "2021"
[workspace]
[dependencies]
rocksdb = {{ package = "ckb-rocksdb", version = "={version}", default-features = false, features = ["snappy", "lz4"] }}
''')
        if args.legacy_lockfiles:
            shutil.copyfile(args.legacy_lockfiles / version / 'Cargo.lock', project / 'Cargo.lock')
        target = args.legacy_target.resolve() if args.legacy_target else output / 'legacy-target'
        if target.is_relative_to(source_root):
            parser.error('legacy build target must be outside the source checkout')
        build_env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0')
        writer = target / 'debug' / ('legacy-hot-db.exe' if os.name == 'nt' else 'legacy-hot-db')
        record = {'binding': version, 'commands': [], 'writer_source_sha256': digest(project / 'src/main.rs'),
                  'manifest_sha256': digest(project / 'Cargo.toml'), 'target': str(target)}
        metadata['runs'].append(record)
        build = ['cargo', 'build', '--manifest-path', str(project / 'Cargo.toml')]
        if args.legacy_lockfiles:
            build.append('--locked')
        if args.offline:
            build.append('--offline')
        run(f'{version}: build', build, build_env, project / 'build.log', record['commands'])
        packages = tomllib.loads((project / 'Cargo.lock').read_text())['package']
        record['native'] = next(p for p in packages if p['name'] == 'ckb-librocksdb-sys')
        record['binding_package'] = next(p for p in packages if p['name'] == 'ckb-rocksdb')
        record['lock_sha256'] = digest(project / 'Cargo.lock')
        record['writer_sha256'] = digest(writer)
        retained_writer = project / writer.name
        shutil.copy2(writer, retained_writer)
        command = ['cargo', 'test', '--locked', '-p', 'ckb-store', '--lib', '--features', 'legacy-db-test']
        if args.offline:
            command.append('--offline')
        command += [test, '--', '--ignored', '--exact', '--nocapture']
        env = dict(consumer_env, CKB_LEGACY_WRITER=str(retained_writer),
                   CKB_UPGRADE_DIR=str(project / 'databases'))
        check = run(f'{version}: upgrade', command, env, project / 'upgrade.log', record['commands'])
        log = (project / 'upgrade.log').read_text()
        check['selected_one_case'] = bool(re.search(r'test result: ok\. 1 passed; 0 failed;', log))
        check['storage_modes'] = [mode for mode in ['sst', 'wal'] if f'{mode}: matched all ' in log]
        save_manifest()
        if not check['selected_one_case'] or check['storage_modes'] != ['sst', 'wal']:
            raise RuntimeError(f'{version}: upgrade filter did not verify both storage modes')
        fixtures = {str(path.relative_to(project)): {'bytes': path.stat().st_size, 'sha256': digest(path)}
                    for path in sorted((project / 'databases').rglob('*')) if path.is_file()}
        fixture_manifest = project / 'fixtures.json'
        fixture_manifest.write_text(json.dumps(fixtures, indent=2) + '\n')
        record['fixture_manifest_sha256'] = digest(fixture_manifest)
        save_manifest()
    if args.candidate:
        run('source-after', verify, os.environ, output / 'source-after.log', metadata['checks'])


if __name__ == '__main__':
    main()
