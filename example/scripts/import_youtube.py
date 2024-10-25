import glob
import json
import os
import subprocess

from dataclasses import dataclass


# Path to top-level directory containing yt-dlp info JSON and subtitle (.vtt)
# downloads. These may be organized into subdirectories as you see fit.
ROOT_DIR = "FIXME_POPULATE_ROOT_DIR"


@dataclass
class Info:
    timestamp: int
    info_path: str
    vtt_path:  str
    tags:      str


infos = []
info_files = glob.glob(f"{ROOT_DIR}/**/*.info.json", recursive=True)
for path in info_files:
    stem = path[:-10]
    vtt = stem + ".ko.vtt"
    for vtt_candidate in glob.glob(f"{glob.escape(stem)}.ko*.vtt"):
        vtt = vtt_candidate
        break
    # print(f"VTT File: {vtt}")
    if not os.path.isfile(vtt):
        continue

    tags = []
    dir = os.path.dirname(path)
    while dir >= ROOT_DIR:
        tags_file = f"{dir}/tags.txt"
        if os.path.isfile(tags_file):
            with open(tags_file, 'r') as f:
                tags.extend([tag.rstrip() for tag in f])
        if dir != ROOT_DIR:
            tag = os.path.basename(dir).replace('_', ' ')
            if tag != "misc":
                tags.append(tag)
        dir = os.path.dirname(dir)
    tags = list(set(tags))

    with open(path) as f:
        info = json.load(f)
        infos.append(Info(
            timestamp=info["timestamp"],
            info_path=path,
            vtt_path=vtt,
            tags=tags,
        ))

infos = sorted(infos, key=lambda info: info.timestamp)


for info in infos:
    # print(f'Importing from: {info.info_path}')
    subprocess.run([
        'target/release/add_book',
        '--db', 'data/prod/lit.db',
        '--config', 'data/prod/config.toml',
        '--unique-url',
        '--allow-duplicate-title',
        '--tags', ','.join(info.tags),
        '--metadata', info.info_path,
        info.vtt_path,
    ])
