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
                    # Check lines before this one, skip attributes
                    j = i - 1
                    # Skip all clippy allowance multi-line macros, or anything starting with #[ or closing ] or inside it
                    # simple heuristic, if it's not a doc string and not empty before pub fn, let's just inspect it manually
                    while j >= 0 and not lines[j].strip().startswith('///'):
                        if "}" in lines[j] or "{" in lines[j] or ";" in lines[j] or lines[j].strip() == "":
                            break
                        j -= 1

                    if j >= 0 and not lines[j].strip().startswith('///'):
                        print(f"{path}:{i+1} -> {s}")

find_undocumented("autumn/src")
