module_name = 1
copy = module_name
del module_name
雪 = 1
copied_unicode = 雪
del 雪
import os.path as path
from os import path as imported
global_name = 1

def globals_and_attributes(obj):
    global global_name
    global_name = obj.attribute
    obj.attribute = global_name
    del obj.attribute
    del global_name

def locals_and_check(flag):
    if flag:
        local_name = 1
    saved = local_name
    del local_name
    return saved

def enclosing(arg):
    closed_name = arg
    def nested():
        nonlocal closed_name
        closed_name = arg
        saved = closed_name
        del closed_name
        return saved
    return nested

def comprehension(seq):
    return [local_name for local_name in seq]

class Base:
    def method(self):
        return 1

class Child(Base):
    def method(self):
        return super().method()

def generic_outer():
    captured = 1
    class Generic[T]:
        type Alias = tuple[T, captured, global_name]
    return Generic
