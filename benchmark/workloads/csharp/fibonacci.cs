static long Fibonacci(long value)
{
    if (value < 2)
    {
        return value;
    }
    return Fibonacci(value - 1) + Fibonacci(value - 2);
}

long total = 0;
for (var index = 0; index < 30; index++)
{
    total += Fibonacci(28);
}
Console.WriteLine(total);
