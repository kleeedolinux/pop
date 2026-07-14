import std.stdio : writeln;

void main() {
    ulong total = 0;
    foreach (index; 1UL .. 20_001UL) {
        auto values = new ulong[](256);
        values[] = index;
        total += values[0];
    }
    writeln(total);
}
