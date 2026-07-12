var values = new long[1_000_000];
for (var index = 0; index < values.Length; index++)
{
    values[index] = index + 1;
}

long total = 0;
for (var index = 0; index < values.Length; index++)
{
    total += values[index];
}
Console.WriteLine(total);
