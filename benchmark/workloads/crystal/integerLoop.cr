index = 1_i64
value = 0_i64
while index <= 50_000_000
  value += index
  index += 1
end
puts value
