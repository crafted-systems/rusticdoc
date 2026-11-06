"""Run inside the container, as the service's user."""
import os, sys
status = 0
found = {}
for pid in filter(str.isdigit, os.listdir('/proc')):
    try:
        cmd = open(f'/proc/{pid}/cmdline', 'rb').read().replace(b'\0', b' ').decode(errors='replace')
    except OSError:
        continue
    if int(pid) == os.getpid() or not cmd:
        continue
    found[pid] = cmd
    label = cmd.split(' ')[0].rsplit('/', 1)[-1]
    if 'rusticdoc' in label and 'serve' in cmd:
        for part in ('environ', 'mem', 'maps'):
            try:
                open(f'/proc/{pid}/{part}', 'rb').read(16)
                print(f'FAIL: the service {pid} {part} is readable')
                status = 1
            except OSError as e:
                print(f'ok: service {pid} {part}: {e.strerror}')
        continue
    try:
        env = open(f'/proc/{pid}/environ', 'rb').read()
    except OSError as e:
        print(f'{label} {pid}: environ unreadable: {e.strerror}')
        continue
    leaked = b'marker' in env
    print(f'{"FAIL" if leaked else "ok"}: {label} {pid}: {len(env.split(bytes(1)))} variables, secrets {"PRESENT" if leaked else "absent"}')
    status |= leaked
print('processes:', sorted(set(c.split(" ")[0].rsplit("/", 1)[-1] for c in found.values())))
if not any('soffice' in c for c in found.values()):
    print('FAIL: no soffice ran, so this proves nothing')
    status = 1
sys.exit(status)
