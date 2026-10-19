global_name = 1
module_name = global_name
del module_name
import os.path as path
from os import path as imported

def enclosing(arg):
    cell = arg
    class Captured:
        saved = cell
        def read(self):
            return cell
    def nested(value):
        nonlocal cell
        cell = value
        saved = cell
        del cell
        return arg, saved
    return Captured, nested

def names(obj, arg):
    global global_name
    global_name = obj.attribute
    obj.attribute = arg
    del obj.attribute
    del global_name
    return obj.method(arg)

def locals_and_check(flag):
    if flag:
        local_name = 1
    saved = local_name
    del local_name
    return saved

def packed(a, b, pair):
    first, second = pair
    saved = a
    return b, saved, first, second

def protected(resource):
    with resource:
        return "with-marker"

def comprehension(seq, cell):
    def read():
        return cell
    result = [cell for cell in seq]
    return result, read

def free_comprehension(cell):
    def nested(seq):
        result = [cell for cell in seq]
        return result, cell
    return nested
