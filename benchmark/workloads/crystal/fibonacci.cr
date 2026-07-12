def fibonacci(value : Int64) : Int64
  return value if value < 2

  fibonacci(value - 1) + fibonacci(value - 2)
end

total = 0_i64
30.times { total += fibonacci(28_i64) }
puts total
