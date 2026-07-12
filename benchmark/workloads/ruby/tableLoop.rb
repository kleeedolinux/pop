values = Array.new(1_000_000)
index = 0
while index < values.length
  values[index] = index + 1
  index += 1
end

total = 0
index = 0
while index < values.length
  total += values[index]
  index += 1
end
puts total
