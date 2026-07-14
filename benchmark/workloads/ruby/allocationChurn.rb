total = 0
index = 1
while index <= 20_000
  values = Array.new(256, index)
  total += values[0]
  index += 1
end
puts total
