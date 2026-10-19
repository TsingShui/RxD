// Original fixture for static metadata/IL queries. Target code is never executed.
using System;
using System.Runtime.InteropServices;

namespace Rxd.Closure;

public interface IContract<T> { T Value { get; } void Accept(T value); }
public abstract class Base {
    public abstract int Abstract(); public int Inherited() => 7;
    [DllImport("fixture-never-loaded")] public static extern int Native(int value);
}
public abstract class Root<T> : Base, IContract<T> where T : class, new()
{
    public T Value { get; set; } = new T();
    public event Action? Changed;
    public void Accept(T value) { Value = value; Changed?.Invoke(); }
    public override int Abstract() => 42;
    public abstract void NoBody();
    public int Guard(int value) { try { return 10 / value; } catch (DivideByZeroException) { return -1; } }
    public class Nested
    {
        public int Field;
        public int Get() => Field;
        public class Deep { public string Text() => "deep λ"; }
    }
}
public class Sibling { public int Separate() => 99; }
public struct Value { public int Field; public int Read() => Field; }
public enum Choice { First = 1, Second = 2 }
