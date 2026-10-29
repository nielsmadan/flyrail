import os
import re
import subprocess
import threading


def check_corpus_limits(root, driver, environment):
    sparse = root / "oversize.tsv"
    with sparse.open("wb") as stream:
        stream.truncate(1024 * 1024 * 1024)
    fifo = root / "stream.tsv"
    os.mkfifo(fifo)
    descriptor = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
    stop = threading.Event()

    def feed():
        chunk = b"x" * 65536
        while not stop.is_set():
            try:
                os.write(descriptor, chunk)
            except BlockingIOError:
                stop.wait(0.001)

    writer = threading.Thread(target=feed)
    writer.start()
    measurements = {}
    try:
        for name, corpus in [("sparse", sparse), ("device", "/dev/zero"), ("fifo", fifo)]:
            destination = root / ("rejected-" + name)
            result = subprocess.run(
                ["/usr/bin/time", "-l", *driver, "differential", "--corpus", str(corpus),
                 "--root", str(destination)],
                capture_output=True, env=environment, timeout=8, check=False,
            )
            assert result.returncode == 1, (name, result)
            assert result.stdout == b"" and b"invalid differential corpus\n" in result.stderr, result
            match = re.search(rb"(\d+)\s+maximum resident set size", result.stderr)
            assert match, result.stderr
            resident = int(match[1])
            assert resident < 96 * 1024 * 1024, (name, resident)
            assert not destination.exists()
            measurements[name] = resident
    finally:
        stop.set()
        writer.join(timeout=5)
        os.close(descriptor)
    assert not writer.is_alive()
    print(f"Differential corpus limits: 1 GiB sparse file, infinite device/FIFO terminate; RSS bytes {measurements}")
