// Original repository fixture source; compiled for static analysis, never executed.
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading.Tasks;

namespace Rxd.Sugar;

public static class Probe
{
    public static async Task<int> AsyncAwait(int value)
    {
        await Task.Yield();
        return value + 1;
    }

    public static IEnumerable<int> Iterator(int count)
    {
        for (int i = 0; i < count; i++)
            yield return i * 2;
    }

    public static Func<int, int> ClosureAndLocalFunction(int seed)
    {
        int AddSeed(int value) => value + seed;
        return value => AddSeed(value) * 2;
    }

    public static int[] Linq(int[] values)
    {
        return (from value in values
                where value > 0
                orderby value
                select value * 2).ToArray();
    }

    public static List<Item> Initializers(int value)
    {
        var array = new[] { value, value + 1 };
        return new List<Item> { new Item { Value = array[0] }, new Item { Value = array[1] } };
    }

    public static string Using(string text)
    {
        using var reader = new StringReader(text);
        return reader.ReadToEnd();
    }

    public static int Lock(object gate, Item item)
    {
        lock (gate)
            return ++item.Value;
    }

    public static int NullPropagation(Item? item) => item?.Value ?? -1;

    public static string Interpolation(string name, int value) => $"{name}: {value:D3}";
}

public sealed class Item
{
    public int Value { get; set; }
}
