import std.stdio : writeln;

void main() {
    auto values = new ulong[](1_000_000);
    foreach (index, ref value; values) {
        value = index + 1;
    }
    ulong total;
    foreach (value; values) {
        total += value;
    }
    writeln(total);
}
