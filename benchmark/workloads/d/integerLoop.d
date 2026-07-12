import std.stdio : writeln;

void main() {
    ulong value;
    for (ulong index = 1; index <= 50_000_000; ++index) {
        value += index;
    }
    writeln(value);
}
