import os
import re

def find_undocumented_pub_fns(directory):
    for root, _, files in os.walk(directory):
        for file in files:
            if not file.endswith('.rs'):
                continue
            path = os.path.join(root, file)
            with open(path, 'r', encoding='utf-8') as f:
                lines = f.readlines()

            for i, line in enumerate(lines):
                if line.strip().startswith('pub fn ') or line.strip().startswith('pub async fn '):
                    # Check the line before it
                    if i > 0:
                        prev_line = lines[i-1].strip()
                        if not prev_line.startswith('///') and not prev_line.startswith('#['):
                            print(f"{path}:{i+1} -> {line.strip()}")
                        elif prev_line.startswith('#['):
                            # Walk up until we don't see `#[`
                            j = i - 1
                            while j >= 0 and lines[j].strip().startswith('#['):
                                j -= 1
                            if j >= 0 and not lines[j].strip().startswith('///'):
                                print(f"{path}:{i+1} -> {line.strip()}")

find_undocumented_pub_fns("autumn/src")
