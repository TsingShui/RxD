// Handcrafted analysis fixture. Compiled, never executed by the test suite.
package fixture;

public class FlowFixture {
    private int value;

    public FlowFixture(int value) { this.value = value; }
    public static int add(int left, int right) { return left + right; }
    public static int absolute(int value) { return value < 0 ? -value : value; }
    public static int sum(int count) {
        int result = 0;
        for (int i = 0; i < count; i++) result += i;
        return result;
    }
    public static long wide(long value, double scale) { return value + (long) scale; }
    public static String text() { return "前端\u0000\ud83d\ude00"; }
    public static int length(int[] values) { return values.length; }
    public static int choose(int value) {
        switch (value) { case 2: return 20; case 100: return 30; default: return 40; }
    }
    public static int dense(int value) {
        switch (value) { case 0: return 20; case 1: return 30; case 2: return 40; default: return 50; }
    }
    public static int guarded(String text) {
        try { return text.length(); } catch (RuntimeException failure) { return -1; }
    }
    public static int[] array(int count) { return new int[count]; }
    public static int element(int[] values, int index) { return values[index]; }
    public static boolean flag(boolean[] values, int index) { return values[index]; }
    public static int narrowBranch(int value) { return value > 0 ? (byte) value : value; }
    public static double root(double value) { return Math.sqrt(value); }
    public static Object merged(boolean flag) { return flag ? "text" : new Object(); }
    public static boolean same(Object left, Object right) { return left == right; }
    public static Object create(int value) { return new FlowFixture(value); }
    public int increment() { return value++; }
    public static void discard(String text) { text.length(); }
    public static synchronized int locked(int value) { return value + 1; }
    public static int block(Object lock, int value) {
        synchronized (lock) { return value + 2; }
    }
}
