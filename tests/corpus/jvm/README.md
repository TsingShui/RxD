# Pinned JVM production corpus

The JARs are published open-source library releases, not synthetic scale inputs.
They are parsed/decompiled statically and never executed. Tests are offline.

| Artifact | Published source | Bytes | SHA-256 | License |
| --- | --- | ---: | --- | --- |
| gson-2.11.0.jar | [Google Gson](https://github.com/google/gson/tree/gson-parent-2.11.0), [Maven artifact](https://repo.maven.apache.org/maven2/com/google/code/gson/gson/2.11.0/gson-2.11.0.jar) | 298435 | 57928d6e5a6edeb2abd3770a8f95ba44dce45f3b23b7a9dc2b309c581552a78b | Apache-2.0; GSON-LICENSE |
| rhino-1.7.15.jar | [Mozilla Rhino](https://github.com/mozilla/rhino/tree/Rhino1_7_15_Release), [Maven artifact](https://repo.maven.apache.org/maven2/org/mozilla/rhino/1.7.15/rhino-1.7.15.jar), [corresponding sources](https://repo.maven.apache.org/maven2/org/mozilla/rhino/1.7.15/rhino-1.7.15-sources.jar) | 1407735 | 2427fdcbc149ca0a25ccfbb7c71b01f39ad42708773a47816cd2342861766b63 | MPL-2.0; embedded LICENSE/NOTICE also extracted here |

Gson's 223 base class entries and 1,210 declared methods (constructors and class
initializers included) were independently checked with Temurin
`javap 21.0.12.1 -p -s -classpath gson-2.11.0.jar <all base classes>`.
Count declaration lines ending in `;` that contain `(`, plus `static {};`;
exclude descriptor lines. These counts are not generated from rxd output.

Selected source expectations are from the published source releases:
GsonBuilder.serializeNulls sets the serializeNulls field and returns this;
JsonPrimitive.isNumber checks instanceof Number; JsonArray.size calls the
backing list's size. Rhino Context.getLanguageVersion returns its version field,
and ScriptRuntime.isJSLineTerminator checks the JavaScript line terminators.

Benchmarks use whole-JAR metadata enumeration, original bytecode extraction,
method Java generation and real atomic output writes. Workload-level results
remain separate. They are not a proof that all methods in these libraries can be
decompiled, or that generated Java is semantically equivalent.
