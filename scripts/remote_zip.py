"""Read members of a zip over HTTP without downloading the whole archive.

ZipFile only needs a seekable file; this one fetches the byte ranges it
reads (the central directory at the end, then each chosen member).
"""
import io
import urllib.request


class RemoteFile(io.RawIOBase):
    def __init__(self, url):
        self.url = url
        self.position = 0
        request = urllib.request.Request(url, method="HEAD")
        with urllib.request.urlopen(request) as response:
            self.size = int(response.headers["Content-Length"])
            if response.headers.get("Accept-Ranges") != "bytes":
                raise OSError(f"{url} does not support range requests")
        self.cache = {}

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=io.SEEK_SET):
        base = {io.SEEK_SET: 0, io.SEEK_CUR: self.position, io.SEEK_END: self.size}[whence]
        self.position = max(0, base + offset)
        return self.position

    def read(self, size=-1):
        if size is None or size < 0:
            size = self.size - self.position
        end = min(self.size, self.position + size)
        if end <= self.position:
            return b""
        request = urllib.request.Request(
            self.url, headers={"Range": f"bytes={self.position}-{end - 1}"}
        )
        with urllib.request.urlopen(request) as response:
            data = response.read()
        self.position += len(data)
        return data

    def readinto(self, buffer):
        data = self.read(len(buffer))
        buffer[: len(data)] = data
        return len(data)
