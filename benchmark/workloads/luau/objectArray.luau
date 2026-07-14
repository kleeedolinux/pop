local values = table.create(200000)
for index = 1, 200000 do values[index] = { value = index } end
local total = 0
for index = 1, #values do total += values[index].value end
print(total)
