#!/usr/bin/env python3
from __future__ import print_function
import os
import sys
import glob
import textwrap
from html.parser import HTMLParser

if sys.version_info < (3, 0, 0):
    print("Requires python 3", file=sys.stderr)
    sys.exit(127)

PREAMBLE = """# CKB JSON-RPC Protocols

<!--**NOTE:** This file is auto-generated from code comments.-->

The RPC interface shares the version of the node version, which is returned in `local_node_info`. The interface is fully compatible between patch versions, for example, a client for 0.25.0 should work with 0.25.x for any x.

Allowing arbitrary machines to access the JSON-RPC port (using the `rpc.listen_address` configuration option) is **dangerous and strongly discouraged**. Please strictly limit the access to only trusted machines.

CKB JSON-RPC only supports HTTP now. If you need SSL, please setup a proxy via Nginx or other HTTP servers.

Subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with `rpc.tcp_listen_address` configuration option) and WebSockets (enable with `rpc.ws_listen_address`).

# JSONRPC Deprecation Process

A CKB RPC method is deprecated in three steps.

First the method is marked as deprecated in the CKB release notes and RPC document. However, the RPC method is still available. The RPC document will have the suggestion of the alternative solutions.

The CKB dev team will disable any deprecated RPC methods starting from the next minor version release. Users can enable the deprecated methods via the config file option rpc.enable_deprecated_rpc.

Once a deprecated method is disabled, the CKB dev team will remove it in a future minor version release.

For example, a method is marked as deprecated in 0.35.0, it can be disabled in 0.36.0 and removed in 0.37.0. The minor versions are released monthly, so there's at least two month buffer for a deprecated RPC method.

"""

TYMETHOD_DOT = 'tymethod.'
RPCERROR_HREF_PREFIX = '../enum.RPCError.html#variant.'


def transform_href(href):
    if href.startswith('../enum.RPCError.html#variant.'):
        return '#error-' + href[len(RPCERROR_HREF_PREFIX):]
    elif ('#' + TYMETHOD_DOT) in href:
        # trait.ChainRpc.html#tymethod.get_block
        return '#method-' + href.split(TYMETHOD_DOT)[-1]

    return href


class MarkdownParser():
    def __init__(self, title_level=0):
        self.chunks = []
        self.title_level = title_level
        self.nested_level = 0
        self.indent_level = 0
        self.is_first_paragraph = False
        self.preserve_whitespaces = False
        self.pending_href = None

    def append(self, text):
        self.chunks.append(text)

    def indent(self, text):
        if self.indent_level > 0:
            self.append(textwrap.indent(text, ' ' * self.indent_level))
        else:
            self.append(text)

    def completed(self):
        return self.nested_level < 0

    def handle_startblock(self, tag):
        if tag in ['p', 'li', 'pre', 'div', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6']:
            self.append("\n")
            if self.indent_level > 0:
                self.append(' ' * self.indent_level)

    def handle_endblock(self, tag):
        if tag in ['p', 'li', 'div', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6']:
            self.append("\n")

    def handle_starttag(self, tag, attrs):
        if tag not in ['hr', 'br']:
            self.nested_level += 1

        self.handle_startblock(tag)
        if tag == 'li':
            self.append('*   ')
            self.indent_level += 4
        elif tag == 'pre':
            self.append("```\n")
            self.preserve_whitespaces = True
        elif tag in ['h1', 'h2', 'h3', 'h4', 'h5', 'h6']:
            self.append((int(tag[1:]) + self.title_level - 1) * '#')
            self.append(' ')
        elif tag in ['strong', 'b']:
            self.append('**')
        elif tag in ['em', 'i']:
            self.append('*')
        elif tag == 'code' and not self.preserve_whitespaces:
            self.append('`')
        elif tag == 'a':
            # ignore the first anchor link in title
            if self.chunks[-1].strip().replace('#', '') != '':
                self.pending_href = transform_href(dict(attrs)['href'])
                self.append('[')

    def handle_endtag(self, tag):
        if tag not in ['hr', 'br']:
            self.nested_level -= 1
        if self.completed():
            return

        self.handle_endblock(tag)
        if tag == 'li':
            self.indent_level -= 4
        elif tag == 'pre':
            self.indent("```\n")
            self.preserve_whitespaces = False
        elif tag in ['strong', 'b']:
            self.append('**')
        elif tag in ['em', 'i']:
            self.append('*')
        elif tag == 'code' and not self.preserve_whitespaces:
            self.append('`')
        elif tag == 'a':
            if self.pending_href is not None:
                self.append('](')
                self.append(self.pending_href)
                self.append(')')
                self.pending_href = None

    def handle_data(self, data):
        if self.nested_level < 0:
            return

        if not self.preserve_whitespaces:
            self.append(' '.join(data.splitlines()))
        else:
            self.indent(data)

    def write(self, file):
        for chunk in self.chunks:
            file.write(chunk)


class RPCVar():
    def __init__(self):
        pass

    def handle_starttag(self, tag, attrs):
        pass

    def handle_endtag(self, tag):
        pass

    def handle_data(self, data):
        pass

    def completed(self):
        False


class RPCMethod():
    def __init__(self, name):
        self.name = name
        self.rpc_var_parser = RPCVar()
        self.doc_parser = None

    def handle_starttag(self, tag, attrs):
        if self.rpc_var_parser is not None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.rpc_var_parser = None
                self.doc_parser = MarkdownParser(title_level=4)
                return

            self.rpc_var_parser.handle_starttag(tag, attrs)
        elif not self.doc_parser.completed():
            self.doc_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.rpc_var_parser is not None:
            self.rpc_var_parser.handle_endtag(tag)
            if self.rpc_var_parser.completed():
                self.add_param(
                    self.rpc_var_parser.name,
                    self.rpc_var_parser.ty
                )
                self.rpc_var_parser = RPCVar()
        elif not self.doc_parser.completed():
            self.doc_parser.handle_endtag(tag)

    def handle_data(self, data):
        if self.rpc_var_parser is not None:
            self.rpc_var_parser.handle_data(data)
        elif not self.doc_parser.completed():
            self.doc_parser.handle_data(data)

    def completed(self):
        return self.doc_parser is not None and self.doc_parser.completed()

    def write(self, file):
        file.write("#### Method `{}`\n".format(self.name))
        # TODO: signature
        # name(a: Type, b: Type) : ReturnType | Error
        if self.doc_parser is not None:
            self.doc_parser.write(file)
            file.write("\n")


class RPCModule(HTMLParser):
    def __init__(self, name):
        super().__init__()
        self.name = name
        self.methods = []
        self.doc_parser = None
        self.active_parser = None

    def handle_starttag(self, tag, attrs):
        if self.active_parser is None:
            if self.doc_parser is None and tag == 'div' and attrs == [("class", "docblock")]:
                self.active_parser = self.doc_parser = MarkdownParser(
                    title_level=3)
            elif tag == 'h3' and ('class', 'method') in attrs:
                id = dict(attrs)['id']
                if id.startswith(TYMETHOD_DOT):
                    self.active_parser = RPCMethod(id[len(TYMETHOD_DOT):])
                    self.methods.append(self.active_parser)
        else:
            self.active_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.active_parser is not None:
            self.active_parser.handle_endtag(tag)
            if self.active_parser.completed():
                self.active_parser = None

    def handle_data(self, data):
        if self.active_parser is not None:
            self.active_parser.handle_data(data)

    def write(self, file):
        file.write("### Module {}\n".format(self.name))
        self.doc_parser.write(file)
        file.write("\n")
        for m in self.methods:
            m.write(file)


class RPCDoc(object):
    def __init__(self):
        self.modules = []
        self.types = dict()

    def collect(self):
        for path in sorted(glob.glob("target/doc/ckb_rpc/module/trait.*Rpc.html")):
            module_name = path.split('.')[1][:-3]
            module = RPCModule(module_name)
            self.modules.append(module)
            with open(path) as file:
                module.feed(file.read())

    def write(self, file):
        file.write(PREAMBLE)
        file.write("\n## RPC Methods\n\n")

        for m in self.modules:
            m.write(file)
            file.write("\n")

        file.write("\n## RPC Errors\n\n")

        file.write("\n## RPC Types\n\n")


def main():
    if not os.path.exists("target/doc/ckb_rpc/module/index.html"):
        print("Please run cargo doc first:\n  cargo doc -p ckb-rpc -p ckb-types -p ckb-fixed-hash -p ckb-fixed-hash-core -p ckb-jsonrpc-types --no-deps", file=sys.stderr)
        sys.exit(128)

    doc = RPCDoc()
    doc.collect()
    doc.write(sys.stdout)


if __name__ == '__main__':
    main()
