import std.stdio : writeln;

ulong fibonacci(ulong value) {
    if (value < 2) {
        return value;
    }
    return fibonacci(value - 1) + fibonacci(value - 2);
}

void main() {
    ulong total;
    foreach (_; 0 .. 30) {
        total += fibonacci(28);
    }
    writeln(total);
}
