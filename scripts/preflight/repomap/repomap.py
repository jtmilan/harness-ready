#!/usr/bin/env -S uv run --no-project --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "grep-ast>=0.5",
#   "tree-sitter-language-pack==0.9.0",
#   "tree-sitter>=0.23,<0.25",
#   "networkx>=3.0",
#   "diskcache>=5.6",
# ]
# ///
"""
repomap.py — STANDALONE pre-flight repo-map generator (DESIGN §3.9-A).

A deterministic, LLM-FREE ranked symbol map suitable for prepending to worker
prompts. Vendors a compact RepoMapper-style pipeline:

    git-tracked Rust+JS files
      -> tree-sitter symbol extraction (definitions + references)
      -> file/symbol reference graph
      -> networkx PageRank ranking
      -> binary-search token budget (default 1500)
      -> ranked <repo-map> text to stdout

Cached by `git rev-parse HEAD` + a porcelain-dirty hash, so warm re-runs are
~free (cache hit prints the stored map and exits).

Usage:
    repomap.py <repo_path> [--max-tokens N] [--no-cache] [--stats] [--lang rust,js]
"""
from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from collections import Counter, defaultdict, namedtuple
from pathlib import Path

import networkx as nx
from diskcache import Cache
from grep_ast import filename_to_lang
from grep_ast.tsl import get_language, get_parser

Tag = namedtuple("Tag", ["rel_fname", "fname", "line", "name", "kind"])

# Languages we support. Map filename-derived lang -> our query file stem.
SUPPORTED_LANGS = {
    "rust": "rust",
    "javascript": "javascript",
    # treat jsx/mjs/cjs as javascript for parsing purposes
}
EXT_LANG_OVERRIDE = {
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".jsx": "javascript",
}

QUERIES_DIR = Path(__file__).resolve().parent / "queries"

# ~4 chars per token is the conventional cheap estimate; avoids pulling a
# tokenizer dependency (keeps this LLM-free and light).
CHARS_PER_TOKEN = 4


def estimate_tokens(text: str) -> int:
    return max(1, len(text) // CHARS_PER_TOKEN)


def run_git(repo: Path, *args: str) -> str:
    out = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        check=False,
    )
    return out.stdout


def git_head(repo: Path) -> str:
    return run_git(repo, "rev-parse", "HEAD").strip() or "NO-HEAD"


def porcelain_dirty_hash(repo: Path) -> str:
    """Hash of the porcelain status so uncommitted edits invalidate the cache."""
    status = run_git(repo, "status", "--porcelain")
    return hashlib.sha256(status.encode("utf-8", "replace")).hexdigest()[:16]


# Path fragments that mark vendored / generated / minified code we never want
# to rank — they bury real source symbols under noise and burn the token budget.
VENDOR_MARKERS = (
    "/vendor/", "/node_modules/", "/dist/", "/build/", "/target/",
    "/.min.", ".min.js", ".min.mjs", "/generated/", "/gen/",
)


def is_vendored_or_minified(rel: str, path: Path) -> bool:
    low = "/" + rel.lower()
    if any(m in low for m in VENDOR_MARKERS):
        return True
    # Minification heuristic: very long average line length over a small line
    # count is a strong minified-bundle signal (cheap, deterministic).
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            head = fh.read(200_000)
    except Exception:
        return False
    lines = head.splitlines() or [""]
    avg = len(head) / max(1, len(lines))
    return avg > 400


def list_source_files(repo: Path, langs: set[str]) -> list[Path]:
    """Git-tracked source files in the supported languages (excludes vendor/min)."""
    tracked = run_git(repo, "ls-files").splitlines()
    files: list[Path] = []
    for rel in tracked:
        p = repo / rel
        if not p.is_file():
            continue
        lang = lang_for(rel)
        if not (lang and lang in langs):
            continue
        if is_vendored_or_minified(rel, p):
            continue
        files.append(p)
    return files


def lang_for(fname: str) -> str | None:
    ext = os.path.splitext(fname)[1].lower()
    if ext in EXT_LANG_OVERRIDE:
        return EXT_LANG_OVERRIDE[ext]
    lang = filename_to_lang(fname)
    if lang in SUPPORTED_LANGS:
        return lang
    return None


def load_query(lang: str) -> str | None:
    qf = QUERIES_DIR / f"{lang}-tags.scm"
    if not qf.exists():
        return None
    return qf.read_text(encoding="utf-8")


def get_tags_for_file(repo: Path, path: Path) -> list[Tag]:
    rel = str(path.relative_to(repo))
    lang = lang_for(rel)
    if not lang:
        return []
    query_scm = load_query(lang)
    if not query_scm:
        return []
    try:
        language = get_language(lang)
        parser = get_parser(lang)
    except Exception:
        return []
    try:
        code = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return []
    if not code.strip():
        return []
    tree = parser.parse(code.encode("utf-8", "replace"))
    try:
        query = language.query(query_scm)
    except Exception:
        return []

    tags: list[Tag] = []
    captures = _run_query(query, tree.root_node)
    for node, tag_name in captures:
        if tag_name.startswith("name.definition."):
            kind = "def"
        elif tag_name.startswith("name.reference."):
            kind = "ref"
        else:
            continue
        try:
            name = node.text.decode("utf-8", "replace")
        except Exception:
            continue
        if not name:
            continue
        tags.append(
            Tag(
                rel_fname=rel,
                fname=str(path),
                line=node.start_point[0],
                name=name,
                kind=kind,
            )
        )
    return tags


def _run_query(query, root):
    """Yield (node, capture_name) across tree-sitter API variants.

    tree-sitter 0.23/0.24 + tree-sitter-language-pack 0.9.0 exposes
    Query.captures(root) -> dict[capture_name] -> [nodes]. We also tolerate the
    newer QueryCursor API and the very-old list-of-pairs form.
    """
    caps = None
    if hasattr(query, "captures"):
        try:
            caps = query.captures(root)
        except Exception:
            caps = None
    if caps is None:
        try:
            from tree_sitter import QueryCursor  # type: ignore

            caps = QueryCursor(query).captures(root)
        except Exception:
            caps = {}
    if isinstance(caps, dict):
        for name, nodes in caps.items():
            for node in nodes:
                yield node, name
    else:
        for node, name in caps:
            yield node, name


def compute_pagerank(G, alpha: float = 0.85, max_iter: int = 100,
                      tol: float = 1.0e-6) -> dict:
    """Pure-python weighted PageRank over a MultiDiGraph.

    Avoids networkx's scipy/numpy backend so this stays dependency-light and
    deterministic. Collapses multi-edges into summed weights first.
    """
    if G.number_of_nodes() == 0:
        return {}
    # Collapse multi-edges into a simple weighted adjacency.
    out: dict = defaultdict(lambda: defaultdict(float))
    nodes = list(G.nodes())
    for u, v, data in G.edges(data=True):
        out[u][v] += float(data.get("weight", 1.0))

    n = len(nodes)
    rank = {node: 1.0 / n for node in nodes}
    out_sum = {node: sum(targets.values()) for node, targets in out.items()}
    dangling = [node for node in nodes if out_sum.get(node, 0.0) == 0.0]

    for _ in range(max_iter):
        prev = rank
        rank = dict.fromkeys(nodes, 0.0)
        danglesum = alpha * sum(prev[d] for d in dangling) / n
        base = (1.0 - alpha) / n
        for u in nodes:
            su = out_sum.get(u, 0.0)
            if su > 0.0:
                share = alpha * prev[u]
                for v, w in out[u].items():
                    rank[v] += share * (w / su)
        for node in nodes:
            rank[node] += base + danglesum
        err = sum(abs(rank[node] - prev[node]) for node in nodes)
        if err < n * tol:
            break
    return rank


def build_ranked_map(repo: Path, langs: set[str]) -> tuple[list[Tag], dict]:
    files = list_source_files(repo, langs)
    stats = {"files_scanned": len(files), "defs": 0, "refs": 0}

    defines: dict[str, set[str]] = defaultdict(set)   # symbol -> {files defining}
    references: dict[str, list[str]] = defaultdict(list)  # symbol -> [files referencing]
    definitions: dict[tuple[str, str], list[Tag]] = defaultdict(list)  # (file,sym)->def tags
    file_tags: dict[str, list[Tag]] = defaultdict(list)

    for path in files:
        for tag in get_tags_for_file(repo, path):
            if tag.kind == "def":
                defines[tag.name].add(tag.rel_fname)
                definitions[(tag.rel_fname, tag.name)].append(tag)
                file_tags[tag.rel_fname].append(tag)
                stats["defs"] += 1
            else:
                references[tag.name].append(tag.rel_fname)
                stats["refs"] += 1

    # If nothing references a symbol, self-reference each definer so it still ranks.
    idents = set(defines.keys())
    for ident in idents:
        if ident not in references:
            references[ident] = list(defines[ident])

    # Build a weighted multidigraph: edge from referencer -> definer per shared symbol.
    G = nx.MultiDiGraph()
    for ident in idents:
        definers = defines[ident]
        ref_counts = Counter(references[ident])
        for referencer, num in ref_counts.items():
            for definer in definers:
                # weight scaled by sqrt(num) to dampen mega-callers
                G.add_edge(referencer, definer, weight=num ** 0.5, ident=ident)

    if G.number_of_nodes() == 0:
        return [], stats

    ranked = compute_pagerank(G)

    # Distribute each file's rank across the symbols it defines that are referenced.
    ranked_definitions: dict[tuple[str, str], float] = defaultdict(float)
    for referencer, definer, data in G.edges(data=True):
        ident = data["ident"]
        src_rank = ranked.get(referencer, 0.0)
        ranked_definitions[(definer, ident)] += src_rank * data["weight"]

    # Also give every defined symbol a small floor from its own file rank so
    # files with defs but no inbound refs still appear.
    for (fname, ident), _tags in definitions.items():
        ranked_definitions[(fname, ident)] += ranked.get(fname, 0.0) * 0.01

    # Order definitions by rank desc, then file/line for determinism.
    ordered_keys = sorted(
        ranked_definitions.keys(),
        key=lambda k: (-ranked_definitions[k], k[0], k[1]),
    )

    ordered_tags: list[Tag] = []
    for fname, ident in ordered_keys:
        for tag in sorted(definitions.get((fname, ident), []), key=lambda t: t.line):
            ordered_tags.append(tag)

    stats["ranked_symbols"] = len(ordered_keys)
    return ordered_tags, stats


def render_map(tags: list[Tag], limit: int | None = None) -> str:
    """Render an aider-style grouped map: file header then sorted def lines."""
    by_file: dict[str, list[Tag]] = defaultdict(list)
    order: list[str] = []
    count = 0
    for tag in tags:
        if limit is not None and count >= limit:
            break
        if tag.rel_fname not in by_file:
            order.append(tag.rel_fname)
        by_file[tag.rel_fname].append(tag)
        count += 1

    lines: list[str] = []
    for fname in order:
        lines.append(f"{fname}:")
        seen = set()
        for tag in sorted(by_file[fname], key=lambda t: (t.line, t.name)):
            key = (tag.line, tag.name)
            if key in seen:
                continue
            seen.add(key)
            lines.append(f"│  {tag.name}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def fit_to_budget(tags: list[Tag], max_tokens: int) -> str:
    """Binary-search the number of leading ranked tags that fits the budget."""
    if not tags:
        return ""
    full = render_map(tags)
    if estimate_tokens(full) <= max_tokens:
        return full

    lo, hi = 0, len(tags)
    best = ""
    while lo <= hi:
        mid = (lo + hi) // 2
        candidate = render_map(tags, limit=mid)
        if estimate_tokens(candidate) <= max_tokens:
            best = candidate
            lo = mid + 1
        else:
            hi = mid - 1
    return best


def cache_key(head: str, dirty: str, max_tokens: int, langs: set[str]) -> str:
    raw = f"{head}|{dirty}|{max_tokens}|{','.join(sorted(langs))}"
    return hashlib.sha256(raw.encode()).hexdigest()


def parse_langs(arg: str | None) -> set[str]:
    if not arg:
        return set(SUPPORTED_LANGS.keys())
    out: set[str] = set()
    for tok in arg.split(","):
        tok = tok.strip().lower()
        if tok in ("js", "javascript"):
            out.add("javascript")
        elif tok in ("rs", "rust"):
            out.add("rust")
        elif tok in SUPPORTED_LANGS:
            out.add(tok)
    return out or set(SUPPORTED_LANGS.keys())


def main() -> int:
    ap = argparse.ArgumentParser(description="Standalone pre-flight repo-map generator (LLM-free).")
    ap.add_argument("repo", help="Path to the git repository to map.")
    ap.add_argument("--max-tokens", type=int, default=1500,
                    help="Token budget for the emitted map (default 1500). "
                         "Explicit, deterministic — no aider no-files auto-inflation.")
    ap.add_argument("--lang", default=None, help="Comma list: rust,js (default both).")
    ap.add_argument("--no-cache", action="store_true", help="Skip the SHA-keyed cache.")
    ap.add_argument("--cache-dir", default=None,
                    help="Cache dir (default <repo>/.git/repomap-cache).")
    ap.add_argument("--stats", action="store_true",
                    help="Print scan stats + cache state to stderr.")
    args = ap.parse_args()

    repo = Path(args.repo).resolve()
    if not (repo / ".git").exists() and not run_git(repo, "rev-parse", "--git-dir").strip():
        print(f"error: {repo} is not a git repository", file=sys.stderr)
        return 2

    langs = parse_langs(args.lang)
    head = git_head(repo)
    dirty = porcelain_dirty_hash(repo)
    key = cache_key(head, dirty, args.max_tokens, langs)

    cache_dir = args.cache_dir or str(repo / ".git" / "repomap-cache")
    cache = None if args.no_cache else Cache(cache_dir)

    if cache is not None:
        hit = cache.get(key)
        if hit is not None:
            if args.stats:
                print(f"[repomap] CACHE HIT key={key[:12]} head={head[:12]} "
                      f"dirty={dirty} tokens<={args.max_tokens}", file=sys.stderr)
            sys.stdout.write(hit)
            return 0

    tags, stats = build_ranked_map(repo, langs)
    body = fit_to_budget(tags, args.max_tokens)
    rendered_tokens = estimate_tokens(body) if body else 0

    header = (
        f"<repo-map repo=\"{repo.name}\" head=\"{head[:12]}\" "
        f"max-tokens=\"{args.max_tokens}\" langs=\"{','.join(sorted(langs))}\">\n"
    )
    footer = "</repo-map>\n"
    out = header + body + footer

    if cache is not None:
        cache.set(key, out)

    if args.stats:
        print(f"[repomap] CACHE MISS key={key[:12]} head={head[:12]} dirty={dirty}",
              file=sys.stderr)
        print(f"[repomap] files_scanned={stats['files_scanned']} defs={stats['defs']} "
              f"refs={stats['refs']} ranked_symbols={stats.get('ranked_symbols', 0)} "
              f"emitted_tokens~={rendered_tokens} budget={args.max_tokens}", file=sys.stderr)

    sys.stdout.write(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
