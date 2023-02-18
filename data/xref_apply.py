from ghidra.program.model.symbol import SourceType

import os
import json

def makeSymbol(symbol):
    addr = toAddr(symbol["offset"])
    symbol = symbol["symbol"]
    createLabel(addr, symbol, True, SourceType.USER_DEFINED)


ghidraDataPath = os.path.join(getSourceFile().getParentFile().toString(), "xref_apply.json")
with open(ghidraDataPath, "r") as jsonFile:
    jsonData = json.load(jsonFile)
    for symbol in jsonData["symbols"]:
        makeSymbol(symbol)
