import std.stdio : writeln;

class Box {
    ulong value;

    this(ulong value) {
        this.value = value;
    }
}

void main() {
    auto values = new Box[](200_000);
    foreach (index, ref value; values) {
        value = new Box(index + 1);
    }
    ulong total = 0;
    foreach (value; values) {
        total += value.value;
    }
    writeln(total);
}
