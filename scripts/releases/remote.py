"""Small GitHub/OCI HTTP boundary. Importing this module performs no requests."""
import base64
import hashlib
import json
from pathlib import PurePosixPath
import re
import urllib.error
import urllib.parse
import urllib.request
import zipfile


class APIError(RuntimeError):
    def __init__(self, status, operation):
        super().__init__(f'{operation}: HTTP {status}')
        self.status = status


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        parsed = urllib.parse.urlparse(newurl)
        if parsed.scheme != 'https':
            raise ValueError('Refusing a non-HTTPS redirect')
        redirected = super().redirect_request(request, fp, code, message, headers, newurl)
        if parsed.hostname != urllib.parse.urlparse(request.full_url).hostname:
            redirected.remove_header('Authorization')
        return redirected


def request(url, method='GET', data=None, headers=None, missing=False):
    req = urllib.request.Request(url, method=method, data=data, headers=headers or {})
    try:
        with urllib.request.build_opener(SafeRedirect()).open(req, timeout=120) as response:
            return response.read(), dict(response.headers)
    except urllib.error.HTTPError as error:
        if missing and error.code == 404:
            return None, {}
        # Never print upload-session URLs, response bodies, or credentials.
        raise APIError(error.code, method + ' ' + urllib.parse.urlparse(url).hostname) from None


def unzip(download, destination):
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(download) as archive:
        seen = set()
        for entry in archive.infolist():
            path = PurePosixPath(entry.filename)
            if (entry.filename in seen or path.is_absolute() or '..' in path.parts or '\\' in entry.filename
                    or (entry.external_attr >> 16) & 0o170000 == 0o120000 or entry.file_size > 512 * 1024 * 1024):
                raise ValueError('Unsafe/duplicate/oversized artifact ZIP member')
            seen.add(entry.filename)
        archive.extractall(destination)


class GitHub:
    def __init__(self, repository, token):
        if not re.fullmatch('[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository) or not token:
            raise ValueError('Repository and GitHub token are required')
        self.repository = repository
        self.token = token
        self.prefix = '/repos/' + repository

    def call(self, path, method='GET', payload=None, missing=False):
        if not path.startswith(self.prefix + '/'):
            raise ValueError('Unexpected GitHub API path')
        data, _ = request('https://api.github.com' + path, method,
                          None if payload is None else json.dumps(payload).encode(),
                          {'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
                           'X-GitHub-Api-Version': '2022-11-28', 'Content-Type': 'application/json'}, missing)
        return None if data is None else json.loads(data)

    def pages(self, path, key=None):
        result = []
        for page in range(1, 1001):
            separator = '&' if '?' in path else '?'
            body = self.call(path + f'{separator}per_page=100&page={page}')
            entries = body[key] if key else body
            result.extend(entries)
            if len(entries) < 100:
                return result
        raise ValueError('Pagination exceeded the release tooling limit')

    def download(self, path, destination):
        if not path.startswith(self.prefix + '/'):
            raise ValueError('Unexpected download API path')
        # Actions artifact downloads use the JSON API media type before redirecting
        # to the ZIP. Release assets need octet-stream to return file bytes.
        accept = ('application/octet-stream' if path.startswith(self.prefix + '/releases/assets/')
                  else 'application/vnd.github+json')
        data, _ = request('https://api.github.com' + path,
                          headers={'Authorization': 'Bearer ' + self.token, 'Accept': accept,
                                   'X-GitHub-Api-Version': '2022-11-28'})
        destination.write_bytes(data)

    def upload(self, release, path):
        base = release['upload_url'].split('{', 1)[0]
        if not base.startswith('https://uploads.github.com/repos/' + self.repository + '/releases/'):
            raise ValueError('Unexpected release upload endpoint')
        data, _ = request(base + '?' + urllib.parse.urlencode({'name': path.name}), 'POST', path.read_bytes(),
                          {'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/octet-stream',
                           'X-GitHub-Api-Version': '2022-11-28'})
        return json.loads(data)


MANIFEST_TYPES = ('application/vnd.oci.image.index.v1+json', 'application/vnd.oci.image.manifest.v1+json',
                  'application/vnd.docker.distribution.manifest.list.v2+json',
                  'application/vnd.docker.distribution.manifest.v2+json')


class Registry:
    def __init__(self, image, username, token):
        if not re.fullmatch(r'ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+', image):
            raise ValueError('Expected a lowercase GHCR repository')
        self.image = image
        self.name = image.removeprefix('ghcr.io/')
        query = urllib.parse.urlencode({'service': 'ghcr.io', 'scope': f'repository:{self.name}:pull,push'})
        auth = base64.b64encode(f'{username}:{token}'.encode()).decode()
        raw, _ = request('https://ghcr.io/token?' + query, headers={'Authorization': 'Basic ' + auth})
        self.token = json.loads(raw)['token']

    def call(self, suffix, method='GET', data=None, content_type=None, missing=False):
        headers = {'Authorization': 'Bearer ' + self.token, 'Accept': ', '.join(MANIFEST_TYPES)}
        if content_type:
            headers['Content-Type'] = content_type
        return request('https://ghcr.io/v2/' + self.name + '/' + suffix, method, data, headers, missing)

    def manifest(self, reference, missing=False):
        raw, _ = self.call('manifests/' + urllib.parse.quote(reference, safe=':'), missing=missing)
        if raw is None:
            return None
        value = 'sha256:' + hashlib.sha256(raw).hexdigest()
        if reference.startswith('sha256:') and reference != value:
            raise ValueError('Registry returned different manifest bytes for a digest')
        return {'digest': value, 'raw': raw, 'data': json.loads(raw)}

    def put_manifest(self, reference, raw, media_type):
        self.call('manifests/' + urllib.parse.quote(reference, safe=':'), 'PUT', raw, media_type)

    def blob(self, digest):
        return self.call('blobs/' + digest)[0]

    def put_blob(self, digest, data):
        if self.call('blobs/' + digest, method='HEAD', missing=True)[0] is not None:
            return
        _, headers = self.call('blobs/uploads/', 'POST', b'')
        location = headers.get('Location') or headers.get('location')
        if location and location.startswith('/'):
            location = 'https://ghcr.io' + location
        if not location or urllib.parse.urlparse(location).hostname != 'ghcr.io' or not location.startswith('https://'):
            raise ValueError('Unexpected registry upload location')
        separator = '&' if '?' in location else '?'
        request(location + separator + urllib.parse.urlencode({'digest': digest}), 'PUT', data,
                {'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/octet-stream'})
