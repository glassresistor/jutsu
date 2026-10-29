"""Structured template generator for differential parser fuzzing.

Emits grammatically-plausible templates that still contain nasty corners
(nested precedence, chained filters/tests, slices with all three fields,
kwargs in tests, namespace assignment targets, whitespace-control markers,
deep nesting).  Deterministic given a seed.
"""
import random


class Gen:
    def __init__(self, rng=None):
        self.rng = rng or random.Random()

    # low-level ---------------------------------------------------------
    def pick(self, *opts):
        return self.rng.choice(list(opts))

    def ws(self):
        r = self.rng.random()
        if r < 0.7:
            return " "
        if r < 0.82:
            return "-"
        if r < 0.92:
            return "~"
        return ""

    # expressions -------------------------------------------------------
    def lit(self):
        r = self.rng.random()
        if r < 0.28:
            return str(self.rng.randint(-5, 100))
        if r < 0.45:
            return "{:.4f}".format(self.rng.uniform(-3, 3))
        if r < 0.72:
            q = self.pick("'", '"')
            inner = self.pick("a", "b c", "", "dÃ¶", "quote", "it\'s")
            # strip any occurrence of the delimiter from the content
            inner = inner.replace("'", "").replace('"', "")
            return q + inner + q
        return self.pick("true", "false", "none", "True", "False", "None")

    def name(self):
        return self.pick("a", "b", "x", "y", "ns", "loop", "foo", "bar")

    def expr(self, d=0):
        if d >= 4:
            return self.lit()
        c = self.rng.randrange(16)
        sub = lambda: self.expr(d + 1)
        if c == 0:
            return self.name()
        if c == 1:
            op = self.pick("+", "-", "*", "/", "//", "%", "**")
            pad = "" if op == "**" else " "
            return f"{sub()} {op}{pad}{sub()}"
        if c == 2:
            return f"{self.pick('-', '~')}{sub()}"
        if c == 3:
            op = self.pick("==", "!=", "<", ">", ">=", "<=")
            return f"{sub()} {op} {sub()}"
        if c == 4:
            return f"{sub()} ~ {sub()}"
        if c == 5:
            return f"{sub()} in {sub()}"
        if c == 6:
            return f"{sub()} if {sub()} else {sub()}"
        if c == 7:
            n = self.rng.randint(0, 3)
            items = ", ".join(self.expr(d + 1) for _ in range(n))
            trail = ", " * (n and self.rng.random() < 0.3)
            return f"({items}{trail})" if n else "()"
        if c == 8:
            items = ", ".join(self.expr(d + 1) for _ in range(self.rng.randrange(4)))
            return f"[{items}]"
        if c == 9:
            n = self.rng.randrange(3)
            kv = ", ".join(f"{self.lit()} : {self.expr(d+1)}" for _ in range(n))
            return "{" + kv + "}"
        if c == 10:
            out = self.name()
            for _ in range(self.rng.randrange(1, 4)):
                r = self.rng.random()
                if r < 0.3:
                    out += "." + self.name()
                elif r < 0.55:
                    out += f"[{self.expr(d+1)}]"
                elif r < 0.68:
                    out += (f"[{self.expr(d+1)}:{self.expr(d+1)}:"
                           f"{self.expr(d+1)}]")
                elif r < 0.85:
                    args = ", ".join(self.expr(d + 1) for _ in range(self.rng.randrange(3)))
                    out += f"({args})"
                else:
                    kvs = ", ".join(f"{self.name()}={self.expr(d+1)}"
                                    for _ in range(self.rng.randrange(2)))
                    out += f"({kvs})" if kvs else "(x=1)"
            return out
        if c == 11:
            out = self.name()
            for _ in range(self.rng.randrange(1, 4)):
                fname = self.pick("upper", "join", "replace")
                args = ", ".join(self.expr(d + 1) for _ in range(self.rng.randrange(3)))
                out += f"|{fname}({args})" if args else f"|{fname}"
            return out
        if c == 12:
            test = self.pick("defined", "undefined", "even", "odd", "string", "in")
            neg = "not " if self.rng.random() < 0.4 else ""
            args = ""
            if self.rng.random() < 0.5:
                args = "(" + ", ".join(self.expr(d + 1) for _ in range(2)) + ")"
            elif self.rng.random() < 0.3:
                args = " " + self.expr(d + 1)
            return f"{self.name()} is {neg}{test}{args}"
        if c == 13:
            a, b = sub(), sub()
            joiner = " and " if self.rng.random() < 0.5 else " or "
            return f"({a}){joiner}({b})"
        if c == 14:
            return " ".join(self.lit() for _ in range(self.rng.randrange(2, 4)))
        return f"{self.name()}.{self.name()}"

    # statements --------------------------------------------------------
    def body_of(self, d):
        n = self.rng.randrange(3)
        return "".join(self.stmt(d + 1) for _ in range(n))

    def stmt(self, d=0):
        c = self.rng.random()
        if c < 0.26:
            return self.pick("hello ", "text x", "<html>", "\n", " ", "|", "-")
        if c < 0.40:
            begin = "{{"
            if self.rng.random() < 0.35:
                begin = "{%" + self.pick("-", "~") + "{"
            end = "}}"
            if self.rng.random() < 0.30:
                end = "%" + self.pick("-", "~") + "}"
            inner = self.ws() + self.expr(d + 1) + self.ws()
            return begin + inner + end
        if c < 0.45:
            tgt = self.pick("v", "ns.attr", "(a, b)", "[x, y]", "((a, b), c)")
            rhs = self.expr(d + 1)
            begin = "{%-" if self.rng.random() < 0.2 else "{%"
            endm = "-%}" if self.rng.random() < 0.2 else "%}"
            return f"{begin} set {tgt} = {rhs}{endm}"
        if c < 0.47:
            tgt = self.pick("v", "(a, b)")
            return "{% set " + tgt + " %}" + self.body_of(d) + "{% endset %}"
        if c < 0.53:
            inner = " for a in " + self.expr(d + 1)
            if self.rng.random() < 0.3:
                inner += " if c"
            if self.rng.random() < 0.2:
                inner += " recursive"
            out = "{%" + inner + "%}" + self.body_of(d)
            if self.rng.random() < 0.4:
                out += "{% else %}" + self.body_of(d)
            out += "{% endfor %}"
            return out
        if c < 0.58:
            out = "{% if " + self.expr() + "%}" + self.body_of(d)
            if self.rng.random() < 0.4:
                out += "{% elif " + self.expr() + "%}" + self.body_of(d)
            if self.rng.random() < 0.5:
                out += "{% else %}" + self.body_of(d)
            out += "{% endif %}"
            return out
        if c < 0.62:
            name = self.pick("base", "content", "footer")
            mods = " scoped" if self.rng.random() < 0.3 else ""
            body = self.body_of(d)
            close = f"{{% endblock {name} %}}" if self.rng.random() < 0.4 else "{% endblock %}"
            return "{% block " + name + mods + " %}" + body + close
        if c < 0.64:
            return "{% extends " + self.expr() + "%}"
        if c < 0.66:
            inc = self.expr()
            mod = self.pick("", " ignore missing", " ignore missing without context")
            seq = ""
            if self.rng.random() < 0.3:
                items = ", ".join(self.lit() for _ in range(2))
                seq = f" {{{items}}}" if self.rng.random() < 0.5 else f" [{items}]"
            return "{% include " + inc + mod + seq + " %}"
        if c < 0.68:
            ctx = self.pick("", " with context", " without context")
            return "{% import " + self.expr() + " as m" + ctx + " %}"
        if c < 0.695:
            names = ", ".join(self.pick("a", "b as c", "d as e")
                             for _ in range(self.rng.randrange(1, 3)))
            ctx = self.pick("", " without context")
            return "{% from " + self.lit() + " import " + names + ctx + " %}"
        if c < 0.715:
            params = self.macro_params()
            return "{% macro m(" + params + ") %}" + self.body_of(d) + "{% endmacro %}"
        if c < 0.73:
            params = self.macro_params()
            target = "t()" if self.rng.random() < 0.5 else "a.b(x)"
            return "{% call(" + params + ") " + target + " %}" + self.body_of(d) + "{% endcall %}"
        if c < 0.745:
            chain = self.pick("upper", "upper|lower", "join('')")
            return "{% filter " + chain + " %}" + self.body_of(d) + "{% endfilter %}"
        if c < 0.76:
            inner = ", ".join(f"{self.name()}={self.expr()}"
                             for _ in range(self.rng.randrange(2)))
            return "{% with " + inner + " %}" + self.body_of(d) + "{% endwith %}"
        if c < 0.775:
            val = self.pick("true", "false")
            return "{% autoescape " + val + " %}" + self.body_of(d) + "{% endautoescape %}"
        if c < 0.81:
            note = self.pick("", "note", "note with quotes ")
            return "{#" + note + "#}"
        if c < 0.835:
            return "{% raw %}" + "{{" + self.expr() + "}}" + "{% endraw %}"
        if c < 0.885:
            args = ", ".join(self.expr() for _ in range(self.rng.randrange(1, 3)))
            return "{% print " + args + " %}"
        return self.pick("\n", "\t", " ", "~\n", "<p>hi</p>", "%")

    def macro_params(self):
        parts = []
        n = self.rng.randrange(3)
        for _ in range(n):
            if self.rng.random() < 0.5:
                parts.append(f"{self.name()}={self.expr()}")
            else:
                parts.append(self.name())
        if self.rng.random() < 0.25:
            parts.append("*" + self.name())
        if self.rng.random() < 0.2:
            parts.append("**k")
        return ", ".join(parts)

    def template(self):
        n = self.rng.randrange(1, 8)
        return "".join(self.stmt(0) for _ in range(n))
