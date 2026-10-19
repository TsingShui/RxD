package fixture;

public class ModernFixture {
    public static String concat(int value) { return "value=" + value; }
    public static Runnable lambda() { return () -> System.out.println("fixture"); }
}

interface DefaultParent {
    default int value() { return 7; }
}

class InterfaceSuperFixture implements DefaultParent {
    public int call() { return DefaultParent.super.value(); }
}
