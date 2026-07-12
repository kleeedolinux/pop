def fibonacci(value)
  return value if value < 2

  fibonacci(value - 1) + fibonacci(value - 2)
end

total = 0
30.times { total += fibonacci(28) }
puts total
