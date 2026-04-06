import os

def find_undocumented(directory):
    for root, _, files in os.walk(directory):
        for file in files:
            if not file.endswith('.rs'):
                continue
            path = os.path.join(root, file)
            with open(path, 'r', encoding='utf-8') as f:
                lines = f.readlines()

            for i, line in enumerate(lines):
                s = line.strip()
                if s.startswith('pub fn ') or s.startswith('pub async fn '):
                    # Check lines before this one
                    j = i - 1
                    has_doc = False
                    while j >= 0:
                        line_j = lines[j].strip()
                        if line_j.startswith('///'):
                            has_doc = True
                            break
                        if "{" in line_j or "}" in line_j or ";" in line_j or (line_j == '' and j != i - 1):
                            break
                        j -= 1

                    if not has_doc:
                        print(f"{path}:{i+1} -> {s}")

find_undocumented("autumn-harvest/autumn-harvest/src")
