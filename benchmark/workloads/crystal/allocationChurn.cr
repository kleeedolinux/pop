total = 0_i64
index = 1_i64
while index <= 20_000
  values = Array(Int64).new(256, index)
  total += values[0]
  index += 1
end
puts total
