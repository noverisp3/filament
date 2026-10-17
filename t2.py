def esc(s):
    out='"'
    for c in s:
        if c=='"': out+='\\"'
        elif c=='\\': out+='\\\\'
        elif c=='\n': out+='\\n'
        elif c=='\r': out+='\\r'
        elif c=='\t': out+='\\t'
        elif ord(c)<0x20: out+='\\u%04x'%ord(c)
        else: out+=c
    return out+'"'
desc="say \"hi\"\nnow"
out = '{{"version":{{"name":{},"protocol":{}}},\n"players":{{"max":{},"online":{}}},\n"description":{{"text":{}}}}}'.format(esc("v"),777,20,0,esc(desc))
print(repr(out))
print(out)
