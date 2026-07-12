local values = table.create(1000000)
for index = 1, 1000000 do values[index] = index end
local total = 0
for index = 1, #values do total += values[index] end
print(total)
