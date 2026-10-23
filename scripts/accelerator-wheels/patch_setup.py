"""Writes a patched copy of an add-on's setup.py for the wheel builds.

    python patch_setup.py <sage2|sage3|flash|nunchaku> <setup.py> <output>
        [--threads N] [--cc 12.0] [--local-version cu130torch2141]

Used by build.sh (Linux) and the Windows wheel workflow, so both apply the
same changes. The source checkouts stay untouched.

- All: -std=c++17 -> -std=c++20 (torch >= 2.13 headers require C++20).
- sage2: nvcc --threads; the Hopper extension (_qattn_sm90) is built for
  sm_90a only. Upstream gives every extension the full -gencode list, and
  sm_90a instructions don't compile for the other targets.
- sage3: nvcc --threads; the target compute capability replaces the GPU query
  (build machines have no GPU); optional local version label, as used by the
  Windows wheel names (sageattn3-1.0.0+cu130torch291).
- flash: nothing else (it reads NVCC_THREADS itself).
- nunchaku: nvcc --threads (upstream uses one thread per target).
"""

import argparse
import sys

parser = argparse.ArgumentParser()
parser.add_argument("package", choices=["sage2", "sage3", "flash", "nunchaku"])
parser.add_argument("src")
parser.add_argument("dst")
parser.add_argument("--threads", default="2")
parser.add_argument("--cc", default="12.0", help="sage3 target compute capability")
parser.add_argument("--local-version", help="sage3 local version label")
args = parser.parse_args()

text = open(args.src, encoding="utf-8").read()


def replace(old, new, count=1):
    """Replaces old with new; exits unless old occurs exactly count times."""
    global text
    found = text.count(old)
    if found != count:
        sys.exit(f"patch_setup {args.package}: expected {count} of {old!r}, found {found}")
    text = text.replace(old, new)


text = text.replace("-std=c++17", "-std=c++20")

if args.package == "sage2":
    replace('"--threads=8"', f'"--threads={args.threads}"')
    replace(
        "    # Fused kernels and QAttn variants\n",
        '''    def gencode_only(flags, num):
        """NVCC flags with every -gencode pair dropped except compute_<num>."""
        kept, i = [], 0
        while i < len(flags):
            if flags[i] == "-gencode":
                if flags[i + 1].startswith(f"arch=compute_{num},"):
                    kept += flags[i : i + 2]
                i += 2
            else:
                kept.append(flags[i])
                i += 1
        return kept

    # Fused kernels and QAttn variants
''',
    )
    replace(
        """                extra_compile_args={"cxx": CXX_FLAGS, "nvcc": NVCC_FLAGS},
                extra_link_args=['-lcuda'],""",
        """                extra_compile_args={"cxx": CXX_FLAGS, "nvcc": gencode_only(NVCC_FLAGS, "90a")},
                extra_link_args=['-lcuda'],""",
    )
elif args.package == "sage3":
    major, minor = args.cc.split(".")
    replace('["--threads", "4"]', f'["--threads", "{args.threads}"]')
    replace(
        "cc_major, cc_minor = torch.cuda.get_device_capability()",
        f"cc_major, cc_minor = ({int(major)}, {int(minor)})",
    )
    if args.local_version:
        replace('version="1.0.0"', f'version="1.0.0+{args.local_version}"')
elif args.package == "nunchaku":
    replace('f"--threads={len(sm_targets)}"', f'"--threads={args.threads}"')

open(args.dst, "w", encoding="utf-8").write(text)
