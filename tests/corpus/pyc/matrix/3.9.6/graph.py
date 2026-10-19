"""Pinned synthetic diagnosis fixture; never imported or executed."""
marker = "closure-marker"
raw = b"\xff\x00raw"
surrogate = "\ud800"
unicode_text = "雪☃"
transform = lambda value: ("lambda-marker", value)

def same(x):
    return len(x) + 1

def outer(arg, /, named=1, *, kw=2):
    cell = "shared-marker"
    def same(value):
        nonlocal cell
        cell = value
        return arg, cell, kw
    return same

class Base:
    def method(self):
        return "base-marker"

class Derived(Base):
    def method(self):
        return super().method()

def names(obj, arg):
    global marker
    marker = "changed-marker"
    obj.attr = arg
    return obj.method(arg), marker

def branch(a, b):
    c = a
    d = b
    if c < d:
        return c + d
    return c - d

def protected(value):
    try:
        return value[0]
    except (IndexError, KeyError) as error:
        return str(error)
    finally:
        value.clear()

def loops(seq):
    total = 0
    for value in seq:
        if value:
            total += value
    return ["item-marker" for value in seq], total

def gen(seq):
    yield from seq

async def coro(value):
    return await value
