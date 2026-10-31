"""Collect rust-evaluator fallback reasons over the macro fuzz corpus."""
import random
import sys
from collections import Counter

sys.path.insert(0, "src")
sys.path.insert(0, "tests")

from jutsu import _jutsu, runtime as rt, rust_eval
from jutsu.environment import Environment
from macro_fuzz import MGen, make_vars

REASONS = Counter()


def probe(env, src, vars_):
    toks, lexerr = rust_eval.wrapped_tokens(env, src, None, None)
    ctx = rust_eval.make_context(env, vars_, None)
    try:
        res = _jutsu.eval_template(toks, lexerr, ctx, rt, None)
    except Exception:
        return
    if res[0] == "__unsupported__":
        REASONS[res[1]] += 1


def main(seeds):
    for seed in seeds:
        rng = random.Random(seed)
        g = MGen(rng)
        for _ in range(3000):
            src = g.template()
            env = Environment()
            probe(env, src, make_vars())
    for reason, n in REASONS.most_common():
        print(n, reason)


if __name__ == "__main__":
    main(tuple(int(x) for x in sys.argv[1:]) or (1, 2, 3))
