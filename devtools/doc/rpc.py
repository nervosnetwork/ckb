#!/usr/bin/env python3
from __future__ import print_function
import os
import io
import sys
import glob
import textwrap
import re
from html.parser import HTMLParser

if sys.version_info < (3, 0, 0):
    print("Requires python 3", file=sys.stderr)
    sys.exit(127)

PREAMBLE = """# CKB JSON-RPC Protocols

<!--**NOTE:** This file is auto-generated from code comments.-->

The RPC interface shares the version of the node version, which is returned in `local_node_info`. The interface is fully compatible between patch versions, for example, a client for 0.25.0 should work with 0.25.x for any x.

Allowing arbitrary machines to access the JSON-RPC port (using the `rpc.listen_address` configuration option) is **dangerous and strongly discouraged**. Please strictly limit the access to only trusted machines.

CKB JSON-RPC only supports HTTP now. If you need SSL, please set up a proxy via Nginx or other HTTP servers.

Subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with `rpc.tcp_listen_address` configuration option) and WebSockets (enable with `rpc.ws_listen_address`).

## JSONRPC Deprecation Process

A CKB RPC method is deprecated in three steps.

First, the method is marked as deprecated in the CKB release notes and RPC document. However, the RPC method is still available. The RPC document will have the suggestion of alternative solutions.

The CKB dev team will disable any deprecated RPC methods starting from the next minor version release. Users can enable the deprecated methods via the config file option rpc.enable_deprecated_rpc.

Once a deprecated method is disabled, the CKB dev team will remove it in a future minor version release.

For example, a method is marked as deprecated in 0.35.0, it can be disabled in 0.36.0 and removed in 0.37.0. The minor versions are released monthly, so there's at least a two-month buffer for a deprecated RPC method.

## Minimum Supported Rust Version policy (MSRV)

The crate `ckb-rpc`'s minimum supported rustc version is 1.61.0.

"""

PENDING_TYPES = set()

TYMETHOD_DOT = 'tymethod.'
HREF_PREFIX_RPCERROR = '../enum.RPCError.html#variant.'
RUST_DOC_PREFIX = 'https://doc.rust-lang.org/1.61.0'

NAME_PREFIX_SELF = '(&self, '

CAMEL_TO_SNAKE_PATTERN = re.compile(r'(?<!^)(?=[A-Z])')


def camel_to_snake(name):
    return CAMEL_TO_SNAKE_PATTERN.sub('_', name).lower()


def snake_to_camel(name):
    return name.title().replace('_', '')


def transform_href(href):
    if href.startswith(HREF_PREFIX_RPCERROR):
        return '#error-' + href[len(HREF_PREFIX_RPCERROR):].lower()
    elif ('#' + TYMETHOD_DOT) in href:
        # trait.ChainRpc.html#tymethod.get_block
        return '#method-' + href.split(TYMETHOD_DOT)[-1]
    elif href.startswith('type.'):
        type_name = href.split('.')[1]
        return '#type-{}'.format(type_name.lower())
    elif href == 'trait.ChainRpc.html#canonical-chain':
        return '#canonical-chain'
    elif ('struct.' in href or 'enum.' in href) and href.endswith('.html'):
        type_name = href.split('.')[-2]
        return '#type-{}'.format(type_name.lower())

    return href


def write_method_signature(file, method_name, vars):
    if method_name == 'subscribe':
        file.write('* `subscribe(topic)`\n')
        file.write('    * `topic`: `string`\n')
    elif method_name == 'unsubscribe':
        file.write('* `unsubscribe(id)`\n')
        file.write('    * `id`: `string`\n')
    elif len(vars) > 1:
        file.write('* `{}({})`\n'.format(method_name,
                                         ', '.join(v.name for v in vars[:-1])))
        for var in vars[:-1]:
            file.write('    * `{}`: {}\n'.format(var.name, var.ty))
    else:
        file.write('* `{}()`\n'.format(method_name))
    if method_name == 'subscribe':
        file.write('* result: `string`\n')
    else:
        file.write('* result: {}\n'.format(vars[-1].ty))


class MarkdownParser():
    def __init__(self, title_level=0):
        self.chunks = []
        self.title_level = title_level
        self.nested_level = 0
        self.indent_level = 0
        self.is_first_paragraph = False
        self.preserve_whitespaces = False
        self.pending_href = None
        self.table_cols = 0

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
        if tag in ['p', 'li', 'pre', 'div', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'tr']:
            self.append("\n")
            if self.indent_level > 0:
                self.append(' ' * self.indent_level)

    def handle_endblock(self, tag):
        if tag in ['p', 'li', 'div', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'table']:
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
            # The content here will be used in tag a to ignore the first anchor
            self.append(((int(tag[1:]) + self.title_level - 1) * '#') + ' ')
        elif tag in ['strong', 'b']:
            self.append('**')
        elif tag in ['em', 'i']:
            self.append('*')
        elif tag == 'code' and not self.preserve_whitespaces:
            self.append('`')
        elif tag == 'a':
            # ignore the first anchor link in title
            if not self.chunks[-1].startswith('#') or self.chunks[-1].strip().replace('#', '') != '':
                self.pending_href = transform_href(dict(attrs)['href'])
                self.append('[')
        elif tag == 'thead':
            self.table_cols = 0
        elif tag == 'tr':
            self.append('| ')
        elif tag == 'th' or tag == 'td':
            self.append(' ')

    def handle_endtag(self, tag):
        if tag not in ['hr', 'br']:
            self.nested_level -= 1
        if self.completed():
            return

        self.handle_endblock(tag)

        if tag == 'li':
            self.indent_level -= 4
        elif tag == 'pre':
            if not self.chunks[-1].endswith('\n'):
                self.indent('\n')
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
        elif tag == 'thead':
            self.append("\n")
            if self.indent_level > 0:
                self.append(' ' * self.indent_level)
            self.append('| ')
            for i in range(self.table_cols):
                self.append('--- |')
        elif tag == 'th':
            self.table_cols += 1
            self.append(' |')
        elif tag == 'td':
            self.append(' |')

    def handle_data(self, data):
        if self.nested_level < 0:
            return

        if not self.preserve_whitespaces:
            if data != '\n':
                self.append(' '.join(data.splitlines()))
                if data.endswith('\n'):
                    self.append(' ')
        else:
            if self.chunks[-1] == '```\n' and data[0] == '\n':
                data = data[1:]
            self.indent(data)

    def write(self, file):
        file.write('\n'.join(line.rstrip()
                             for line in ''.join(self.chunks).splitlines()))


class RPCVar():
    def __init__(self):
        self.name = ''
        self.ty = None
        self.children = []
        self.completed_children = 0
        pass

    def require_children(self, n):
        while len(self.children) < n:
            self.children.append(RPCVar())

    def handle_starttag(self, tag, attrs):
        attrs_dict = dict(attrs)

        if tag != 'a' or ('title' in attrs_dict and attrs_dict['title'] == 'goto source code'):
            return
        # 1.61.0 new source link style
        if tag == 'a' and (('class', 'srclink') in attrs):
            return


        if self.ty is None:
            self.ty = attrs_dict['href']
            if self.ty.startswith('#'):
                self.ty = None
                return

            if self.ty ==  RUST_DOC_PREFIX + '/std/primitive.unit.html' :
                self.ty = '`null`'
            if self.ty == RUST_DOC_PREFIX + '/std/primitive.bool.html':
                self.ty = '`boolean`'
            if self.ty == RUST_DOC_PREFIX + '/alloc/string/struct.String.html':
                self.ty = '`string`'
            elif self.ty == RUST_DOC_PREFIX + '/core/option/enum.Option.html':
                self.require_children(1)
            elif self.ty ==  RUST_DOC_PREFIX + '/alloc/vec/struct.Vec.html':
                self.require_children(1)
            elif self.ty == RUST_DOC_PREFIX + '/std/collections/hash/map/struct.HashMap.html':
                self.require_children(2)
            elif self.ty == '../../ckb_jsonrpc_types/enum.ResponseFormat.html':
                self.require_children(2)
            elif self.ty == '../../ckb_indexer/service/struct.Pagination.html':
                self.require_children(1)
            elif self.ty.startswith('../'):
                if '/struct.' in self.ty:
                    PENDING_TYPES.add(self.ty)
                    type_name = self.ty.split('/struct.')[1][:-5]
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())
                elif '/type.' in self.ty:
                    PENDING_TYPES.add(self.ty)
                    type_name = self.ty.split('/type.')[1][:-5]
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())
                elif '/enum.' in self.ty:
                    PENDING_TYPES.add(self.ty)
                    type_name = self.ty.split('/enum.')[1][:-5]
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())

            # after 1.56 rustdoc change relative link
            # now relative link do not start with '../'
            elif 'title' in attrs_dict and 'ckb_jsonrpc_types::' in attrs_dict['title']:
                if ('class', 'struct') in attrs and attrs_dict['title'].startswith('struct') and self.ty.startswith('struct.'):
                    type_name = self.ty.split('struct.')[1][:-5]
                    PENDING_TYPES.add('ckb_jsonrpc_types/' + self.ty)
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())
                elif ('class', 'type') in attrs and attrs_dict['title'].startswith('type') and self.ty.startswith('type.'):
                    type_name = self.ty.split('type.')[1][:-5]
                    PENDING_TYPES.add('ckb_jsonrpc_types/' + self.ty)
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())
                elif ('class', 'enum') in attrs and attrs_dict['title'].startswith('enum') and self.ty.startswith('enum.'):
                    type_name = self.ty.split('enum.')[1][:-5]
                    PENDING_TYPES.add('ckb_jsonrpc_types/' + self.ty)
                    self.ty = '[`{}`](#type-{})'.format(type_name,
                                                        type_name.lower())

        else:
            if self.completed_children >= len(self.children):
                print(">>> {} {}[{}] => {} {} {}".format(
                    self.name, self.ty, self.completed_children, self.completed(), tag, attrs))
            self.children[self.completed_children].handle_starttag(tag, attrs)
            if self.children[self.completed_children].completed():
                if self.completed():
                    if self.ty == RUST_DOC_PREFIX + '/core/option/enum.Option.html':
                        self.ty = '{} `|` `null`'.format(self.children[0].ty)
                    elif self.ty == RUST_DOC_PREFIX + '/alloc/vec/struct.Vec.html':
                        self.ty = '`Array<` {} `>`'.format(self.children[0].ty)
                    elif self.ty == RUST_DOC_PREFIX + '/std/collections/hash/map/struct.HashMap.html':
                        self.ty = '`{{ [ key:` {} `]: ` {} `}}`'.format(
                            self.children[0].ty, self.children[1].ty)
                    elif self.ty == '../../ckb_jsonrpc_types/enum.ResponseFormat.html':
                        molecule_name = self.children[1].ty.split(
                            '`](')[0][2:]
                        self.ty = '{} `|` [`Serialized{}`](#type-serialized{})'.format(
                            self.children[0].ty, molecule_name, molecule_name.lower())
                    elif self.ty == '../../ckb_indexer/service/struct.Pagination.html':
                        self.ty = '`Pagination<` {} `>`'.format(self.children[0].ty)
                else:
                    self.completed_children += 1

    def handle_endtag(self, tag):
        pass

    def handle_data(self, data):
        if self.ty is None:
            self.name = self.sanitize_name(data)
            if self.name.endswith(': U256'):
                parts = self.name.split(': ')
                self.name = parts[0]
                self.ty = '[`U256`](#type-u256)'
            if self.name.endswith(': RationalU256'):
                parts = self.name.split(': ')
                self.name = parts[0]
                self.ty = '[`RationalU256`](#type-rationalu256)'

    def completed(self):
        return self.ty is not None and (len(self.children) == 0 or self.children[-1].completed())

    def sanitize_name(self, name):
        name = name.strip()

        if name.startswith(NAME_PREFIX_SELF):
            name = name[len(NAME_PREFIX_SELF):]
        if name.endswith(':'):
            name = name[:-1]
        if name.startswith(', '):
            name = name[2:]

        return name


class RPCMethod():
    def __init__(self, name):
        self.name = name
        self.rpc_var_parser = RPCVar()
        self.parsing_stability = False
        self.doc_parser = None
        self.params = []

    def handle_starttag(self, tag, attrs):
        if self.rpc_var_parser is not None:
            if tag == 'div' and (attrs == [("class", "docblock")] or attrs == [("class", "stab deprecated")]):
                self.rpc_var_parser = None
                self.doc_parser = MarkdownParser(title_level=1)
                if attrs == [("class", "stab deprecated")]:
                    self.doc_parser.append("\n")
                    self.parsing_stability = True
                return

            self.rpc_var_parser.handle_starttag(tag, attrs)
        elif not self.doc_parser.completed():
            self.doc_parser.handle_starttag(tag, attrs)
        elif self.parsing_stability and tag == 'div' and attrs == [("class", "docblock")]:
            self.parsing_stability = False
            self.doc_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.rpc_var_parser is not None:
            self.rpc_var_parser.handle_endtag(tag)
            if self.rpc_var_parser.completed():
                if '->' not in self.rpc_var_parser.name or 'Result' in self.rpc_var_parser.name:
                    self.params.append(self.rpc_var_parser)
                self.rpc_var_parser = RPCVar()
        elif not self.doc_parser.completed():
            self.doc_parser.handle_endtag(tag)

    def handle_data(self, data):
        if self.rpc_var_parser is not None:
            self.rpc_var_parser.handle_data(data)
        elif not self.doc_parser.completed():
            self.doc_parser.handle_data(data)

    def completed(self):
        return self.doc_parser is not None and not self.parsing_stability and self.doc_parser.completed()

    def write(self, file):
        file.write("\n#### Method `{}`\n".format(self.name))
        write_method_signature(file, self.name, self.params)
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
            elif tag == 'div' and ('class', 'method has-srclink') in attrs:
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


class RPCErrorParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.variants = []

        self.module_doc = None
        self.next_variant = None
        self.variant_parser = None

    def handle_starttag(self, tag, attrs):
        if self.module_doc is None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.module_doc = MarkdownParser(title_level=3)
        elif not self.module_doc.completed():
            self.module_doc.handle_starttag(tag, attrs)
        elif self.next_variant is None:
            if tag == 'h3':
                attrs_dict = dict(attrs)
                if 'id' in attrs_dict and attrs_dict['id'].startswith('variant.') and ('class', 'variant small-section-header') in attrs:
                    self.next_variant = attrs_dict['id'].split('.')[1]
        elif self.variant_parser is None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.variant_parser = MarkdownParser(title_level=3)
        else:
            self.variant_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.module_doc is None:
            return
        elif not self.module_doc.completed():
            self.module_doc.handle_endtag(tag)
        elif self.variant_parser is not None:
            self.variant_parser.handle_endtag(tag)
            if self.variant_parser.completed():
                self.variants.append((self.next_variant, self.variant_parser))
                self.next_variant = None
                self.variant_parser = None

    def handle_data(self, data):
        if self.module_doc is None:
            return
        elif not self.module_doc.completed():
            self.module_doc.handle_data(data)
        elif self.variant_parser is not None:
            self.variant_parser.handle_data(data)

    def write(self, file):
        self.module_doc.write(file)
        file.write('\n\n')

        for (name, variant) in self.variants:
            file.write('### Error `{}`\n'.format(name))
            variant.write(file)
            file.write('\n\n')


class EnumSchema(HTMLParser):
    def __init__(self, name):
        super().__init__()
        self.name = name
        self.variants = []
        self.next_variant = None
        self.variant_parser = None

    def handle_starttag(self, tag, attrs):
        if self.next_variant is None:
            if tag == 'h3':
                attrs_dict = dict(attrs)
                if 'id' in attrs_dict and attrs_dict['id'].startswith('variant.') and ('class', 'variant small-section-header') in attrs:
                    self.next_variant = camel_to_snake(
                        attrs_dict['id'].split('.')[1])
        elif self.variant_parser is None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.variant_parser = MarkdownParser(title_level=3)
                self.variant_parser.indent_level = 4
        else:
            self.variant_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.next_variant is not None and self.variant_parser is not None:
            self.variant_parser.handle_endtag(tag)
            if self.variant_parser.completed():
                if self.next_variant not in [v[0] for v in self.variants]:
                    self.variants.append((self.next_variant, self.variant_parser))
                    self.variant_parser = None
                self.next_variant = None


    def handle_data(self, data):
        if self.variant_parser is not None:
            self.variant_parser.handle_data(data)

    def write(self, file):
        if self.name == 'PoolTransactionReject':
            self.write_pool_transaction_reject(file)
            return

        file.write('`{}` is equivalent to `"{}"`.\n\n'.format(
            self.name, '" | "'.join(v[0] for v in self.variants)))

        for (name, v) in self.variants:
            file.write('*   ')
            out = io.StringIO()
            v.write(out)
            variant_doc = out.getvalue().lstrip()
            file.write(variant_doc)
            file.write('\n')
            if '\n' in variant_doc:
                file.write('\n')

    # PoolTransactionReject
    def write_pool_transaction_reject(self, file):
        file.write(
            '`{}` is a JSON object with following fields.\n\n'.format(self.name))

        file.write('*   `type`: `"{}"` - Reject type.\n'.format(
            '" | "'.join(snake_to_camel(v[0]) for v in self.variants)))
        file.write(
            '*   `description`: `string` - Detailed description about why the transaction is rejected.\n\n')
        file.write('Different reject types:\n\n')

        for (name, v) in self.variants:
            file.write('*   `{}`: '.format(snake_to_camel(name)))
            out = io.StringIO()
            v.write(out)
            file.write(out.getvalue().lstrip())
            file.write('\n')


class StructSchema(HTMLParser):
    def __init__(self, name):
        super().__init__()
        self.name = name
        self.fields = []
        self.next_field = None
        self.type_parser = None
        self.field_parser = None

    def handle_starttag(self, tag, attrs):
        if self.next_field is None:
            if tag == 'span':
                attrs_dict = dict(attrs)
                if 'id' in attrs_dict and attrs_dict['id'].startswith('structfield.'):
                    self.next_field = attrs_dict['id'].split('.')[1]
                    self.type_parser = RPCVar()
        elif not self.type_parser.completed():
            self.type_parser.handle_starttag(tag, attrs)
        elif self.field_parser is None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.field_parser = MarkdownParser(title_level=3)
                self.field_parser.indent_level = 4
        else:
            self.field_parser.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.type_parser is not None and not self.type_parser.completed():
            self.type_parser.handle_endtag(tag)
        elif self.field_parser is not None:
            self.field_parser.handle_endtag(tag)
            if self.field_parser.completed():
                self.fields.append(
                    (self.next_field, self.type_parser, self.field_parser))
                self.next_field = None
                self.type_parser = None
                self.field_parser = None

    def handle_data(self, data):
        if self.type_parser is not None and not self.type_parser.completed():
            self.type_parser.handle_data(data)
        elif self.field_parser is not None:
            self.field_parser.handle_data(data)

    def write(self, file):
        if len(self.fields) == 0:
            return

        file.write('#### Fields\n\n')
        file.write(
            '`{}` is a JSON object with the following fields.\n'.format(self.name))

        for t in self.fields:
            file.write('\n*   `{}`: {} - '.format(t[0], t[1].ty))
            out = io.StringIO()
            t[2].write(out)
            file.write(out.getvalue().lstrip())
            file.write('\n')


class RPCType(HTMLParser):
    def __init__(self, name, path):
        super().__init__()
        self.name = name
        self.path = path
        self.module_doc = None

        if '/enum.' in path and self.name != 'RawTxPool':
            self.schema = EnumSchema(self.name)
        elif '/struct.' in path and self.name != 'ProposalShortId':
            self.schema = StructSchema(self.name)
        else:
            self.schema = None

    def handle_starttag(self, tag, attrs):
        if self.module_doc is None:
            if tag == 'div' and attrs == [("class", "docblock")]:
                self.module_doc = MarkdownParser(title_level=3)
        elif not self.module_doc.completed():
            self.module_doc.handle_starttag(tag, attrs)
        elif self.schema is not None:
            self.schema.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.module_doc is None:
            return
        elif not self.module_doc.completed():
            self.module_doc.handle_endtag(tag)
        elif self.schema is not None:
            self.schema.handle_endtag(tag)

    def handle_data(self, data):
        if self.module_doc is None:
            return
        elif not self.module_doc.completed():
            self.module_doc.handle_data(data)
        elif self.schema is not None:
            self.schema.handle_data(data)

    def write(self, file):
        self.module_doc.write(file)
        file.write('\n')

        if self.schema is not None:
            file.write('\n')
            self.schema.write(file)
            file.write('\n')


class DummyRPCType():
    def __init__(self, name, module_doc):
        super().__init__()
        self.name = name
        self.module_doc = module_doc

    def write(self, file):
        file.write('\n')
        file.write(self.module_doc)
        file.write('\n')


class RPCDoc(object):
    def __init__(self):
        self.modules = []
        self.errors = RPCErrorParser()
        self.parsed_types = set()

        self.types = [
            DummyRPCType(
                "SerializedHeader", "This is a 0x-prefix hex string. It is the block header serialized by molecule using the schema `table Header`."),
            DummyRPCType(
                "SerializedBlock", "This is a 0x-prefix hex string. It is the block serialized by molecule using the schema `table Block`."),
            DummyRPCType(
                "U256", "The 256-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON."),
            DummyRPCType(
                "RationalU256", """The ratio which numerator and denominator are both 256-bit unsigned integers.

#### Example

```
{
    "denom": "0x28",
    "numer": "0x1"
}
```
""")
        ]

    def collect(self):
        for path in sorted(glob.glob("target/doc/ckb_rpc/module/trait.*Rpc.html")):
            module_name = path.split('.')[1][:-3]
            module = RPCModule(module_name)
            self.modules.append(module)
            with open(path) as file:
                module.feed(file.read())

        with open('target/doc/ckb_rpc/enum.RPCError.html') as file:
            self.errors.feed(file.read())

        global PENDING_TYPES
        while len(PENDING_TYPES) > 0:
            pending = PENDING_TYPES
            PENDING_TYPES = set()

            for path in pending:
                self.collect_type(path)

        # Referenced by subscription RPC.
        self.collect_type('ckb_jsonrpc_types/struct.PoolTransactionEntry.html')
        self.collect_type('ckb_jsonrpc_types/enum.PoolTransactionReject.html')
        # Referenced by RawTxPool
        self.collect_type('ckb_jsonrpc_types/struct.TxPoolIds.html')
        self.collect_type('ckb_jsonrpc_types/struct.TxPoolEntries.html')
        self.collect_type('ckb_jsonrpc_types/struct.TxPoolEntry.html')
        self.types.sort(key=lambda t: t.name)

    def collect_type(self, path):
        while path.startswith('../'):
            path = path[3:]
        path = 'target/doc/' + path

        if path in self.parsed_types:
            return
        self.parsed_types.add(path)

        if 'ckb_types/packed' in path:
            return
        if path.split('/')[-1] in ['type.Result.html', 'struct.Subscriber.html', 'enum.SubscriptionId.html', 'enum.Topic.html']:
            return

        with open(path) as file:
            content = file.read()

        if 'http-equiv="refresh"' in content:
            path = content.split('0;URL=')[1].split('"')[0]
            return self.collect_type(path)

        name = path.split('.')[1]
        if name not in ['U256', 'RationalU256']:
            parser = RPCType(name, path)
            parser.feed(content)

            self.types.append(parser)

    def write(self, file):
        file.write(PREAMBLE)
        file.write("\n## Table of Contents\n\n")

        file.write("* [RPC Methods](#rpc-methods)\n")
        for m in self.modules:
            file.write(
                "    * [Module {}](#module-{})\n".format(m.name, m.name.lower()))
            for f in m.methods:
                file.write(
                    "        * [Method `{}`](#method-{})\n".format(f.name, f.name.lower()))
        file.write("* [RPC Errors](#rpc-errors)\n")
        file.write("* [RPC Types](#rpc-types)\n")
        for t in self.types:
            file.write(
                "    * [Type `{}`](#type-{})\n".format(t.name, t.name.lower()))

        file.write("\n## RPC Methods\n\n")

        for m in self.modules:
            m.write(file)
            file.write("\n")

        file.write("\n## RPC Errors\n")
        self.errors.write(file)

        file.write("\n## RPC Types\n")
        for ty in self.types:
            file.write("\n### Type `{}`\n".format(ty.name))
            ty.write(file)


def main():
    if not os.path.exists("target/doc/ckb_rpc/module/index.html"):
        print("Please run cargo doc first:\n  cargo doc -p ckb-rpc -p ckb-types -p ckb-fixed-hash -p ckb-fixed-hash-core -p ckb-jsonrpc-types --no-deps", file=sys.stderr)
        sys.exit(128)

    doc = RPCDoc()
    doc.collect()
    doc.write(sys.stdout)


if __name__ == '__main__':
    main()
