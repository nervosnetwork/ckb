#!/usr/bin/env python3
"""Build published old engines and verify hot DB upgrade, archival and restore."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tomllib


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='New directory for sources, logs and DBs')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    repo = Path(__file__).resolve().parents[2]
    metadata = {
        'platform': platform.platform(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
        'consumer_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'consumer_lock_sha256': digest(repo / 'Cargo.lock'),
        'runs': [],
    }
    (output / 'consumer.patch').write_bytes(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=repo))
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
        target = output / 'legacy-target'
        build_env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG='0')
        writer = target / 'debug' / ('legacy-hot-db.exe' if os.name == 'nt' else 'legacy-hot-db')
        record = {'binding': version, 'commands': []}
        metadata['runs'].append(record)
        commands = [
            ('build', ['cargo', 'build', '--manifest-path', str(project / 'Cargo.toml')], build_env),
            ('upgrade', ['cargo', 'test', '--locked', '-p', 'ckb-store', '--lib', '--features',
                         'legacy-db-test', 'tests::archive::old_engine_hot_data_survives_upgrade_archival_and_backup',
                         '--', '--ignored', '--exact', '--nocapture'],
             dict(os.environ, CKB_LEGACY_WRITER=str(writer), CKB_UPGRADE_DIR=str(project / 'databases'))),
        ]
        for name, command, env in commands:
            print(f'{version}: {name}', flush=True)
            with (project / f'{name}.log').open('w') as log:
                result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT)
            record['commands'].append({'command': command, 'exit_code': result.returncode})
            if name == 'build' and result.returncode == 0:
                packages = tomllib.loads((project / 'Cargo.lock').read_text())['package']
                record['native'] = next(p['version'] for p in packages if p['name'] == 'ckb-librocksdb-sys')
                record['writer_sha256'] = digest(writer)
                shutil.copy2(writer, project / writer.name)
            (output / 'manifest.json').write_text(json.dumps(metadata, indent=2) + '\n')
            result.check_returncode()


if __name__ == '__main__':
    main()
