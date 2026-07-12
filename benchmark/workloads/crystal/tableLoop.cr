values = Array(Int64).new(1_000_000, 0_i64)
index = 0
while index < values.size
  values[index] = index.to_i64 + 1
  index += 1
end

total = 0_i64
index = 0
while index < values.size
  total += values[index]
  index += 1
end
puts total
