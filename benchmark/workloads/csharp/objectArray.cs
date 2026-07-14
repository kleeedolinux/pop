var values = new Box[200_000];
for (var index = 0; index < values.Length; index++)
{
    values[index] = new Box(index + 1);
}
long total = 0;
foreach (var value in values)
{
    total += value.Value;
}
Console.WriteLine(total);

sealed class Box
{
    public Box(long value) => Value = value;
    public long Value { get; }
}
