long total = 0;
for (long index = 1; index <= 20_000; index++)
{
    var values = new long[256];
    Array.Fill(values, index);
    total += values[0];
}
Console.WriteLine(total);
