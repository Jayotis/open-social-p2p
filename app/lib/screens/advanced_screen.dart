import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:share_plus/share_plus.dart';

import '../format.dart';
import '../node.dart';

/// Advanced details (ADR 0031 §2): how this phone reaches your node and how
/// the node is doing, in the protocol's own words. The only screen where
/// they appear; everything else uses plain words.
class AdvancedScreen extends StatefulWidget {
  const AdvancedScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<AdvancedScreen> createState() => _AdvancedScreenState();
}

class _AdvancedScreenState extends State<AdvancedScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.checkStatus();
  }

  Future<void> _forget() async {
    final go = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Forget your node?'),
        content: const Text(
          'This phone stops talking to your node and deletes the saved feed. '
          'To revoke its access completely, also run `osp unpair` on the node.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Forget'),
          ),
        ],
      ),
    );
    if (go != true) return;
    await widget.node.forget();
    if (mounted) Navigator.of(context).popUntil((route) => route.isFirst);
  }

  Future<void> _copyReport() async {
    final messenger = ScaffoldMessenger.of(context);
    await Clipboard.setData(ClipboardData(text: bugReport(widget.node)));
    messenger.showSnackBar(
      const SnackBar(
        content: Text(
          'Copied, without addresses. Paste it into your bug report.',
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Advanced details'),
        actions: [
          IconButton(
            tooltip: 'Copy details for a bug report',
            onPressed: _copyReport,
            icon: const Icon(Icons.bug_report_outlined),
          ),
          IconButton(
            onPressed: node.checkStatus,
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final s = node.status;
          final online = node.reachability == Reachability.online;
          final path = switch (s?.path) {
            'direct' => 'Direct',
            'relay' => 'Through a relay',
            _ => 'Not connected',
          };
          return ListView(
            children: [
              _Section('Connection'),
              _Row('Reachable', online ? 'Yes' : 'No'),
              if (!online && node.lastError != null)
                _Row('Why', node.lastError!),
              if (node.lastContact != null)
                _Row('Last reached', formatAgo(node.lastContact!)),
              if (online && s != null) ...[
                _Row(
                  'Path',
                  '$path${s.rttMs != null ? ', ${s.rttMs} ms' : ''}',
                ),
                if (s.path == 'relay')
                  const _Note(
                    'Relays pass encrypted traffic between your phone and your node when '
                    "they can't connect directly. The relay sees both addresses, not the content.",
                  ),
              ],
              _Row(
                'Phone network',
                node.onMobileData ? 'Mobile data' : 'Wi-Fi or other',
              ),
              if (s != null) ...[
                _Section('Your node'),
                _Row('Owner', s.name),
                _Row(
                  'Health',
                  s.problems.isEmpty
                      ? 'No problems'
                      : '${s.problems.length} problem(s)',
                ),
                for (final p in s.problems) _Note('• $p'),
                _Row('Version', s.version),
                _Row('Running for', formatUptime(s.uptimeSecs)),
                _Row('Relay', s.relayConnected ? 'Connected' : 'Not connected'),
                _Row('Following', '${s.follows}'),
                _Row('Followers', '${s.followers}'),
                _Row('Paired devices', '${s.devices}'),
                if (s.diskFreeBytes != null)
                  _Row('Free disk space', formatBytes(s.diskFreeBytes!)),
              ],
              _Section('This phone'),
              _Row('Device ID', '${node.app.deviceId().substring(0, 10)}…'),
              if (node.paired != null) ...[
                _Row('Paired with', "${node.paired!.name}'s node"),
                if (DateTime.tryParse(node.paired!.paired) case final at?)
                  _Row('Paired', formatAgo(at)),
              ],
              Padding(
                padding: const EdgeInsets.all(16),
                child: OutlinedButton(
                  onPressed: _forget,
                  child: const Text('Forget this node'),
                ),
              ),
              _LogView(node: node),
            ],
          );
        },
      ),
    );
  }
}

/// The node's state for a bug report, with addresses and long IDs removed
/// ([scrubAddresses]), so it can be pasted into a public issue.
String bugReport(HomeNode node) {
  final s = node.status;
  final lines = [
    'Reachable: ${node.reachability.name}',
    if (node.lastError != null) 'Last error: ${node.lastError}',
    if (node.lastContact != null)
      'Last reached: ${formatAgo(node.lastContact!)}',
    'Phone network: ${node.onMobileData ? 'mobile data' : 'Wi-Fi or other'}',
    'Phone: ${Platform.operatingSystem} ${Platform.operatingSystemVersion}',
    if (s != null) ...[
      'Node version: ${s.version}',
      'Running for: ${formatUptime(s.uptimeSecs)}',
      'Path: ${s.path}${s.rttMs != null ? ', ${s.rttMs} ms' : ''}',
      'Relay: ${s.relayConnected ? 'connected' : 'not connected'}',
      'Over Tor: ${s.overTor ? 'yes' : 'no'}',
      'Following: ${s.follows}, followers: ${s.followers}',
      'Paired devices: ${s.devices}',
      if (s.diskFreeBytes != null)
        'Free disk: ${formatBytes(s.diskFreeBytes!)}',
      for (final p in s.problems) 'Problem: $p',
    ],
  ];
  return scrubAddresses(lines.join('\n'));
}

class _Section extends StatelessWidget {
  const _Section(this.title);

  final String title;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 24, 16, 8),
      child: Text(title, style: Theme.of(context).textTheme.titleMedium),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row(this.label, this.value);

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 130,
            child: Text(label, style: Theme.of(context).textTheme.bodySmall),
          ),
          Expanded(child: SelectableText(value)),
        ],
      ),
    );
  }
}

class _Note extends StatelessWidget {
  const _Note(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 2, 16, 6),
      child: Text(text, style: Theme.of(context).textTheme.bodySmall),
    );
  }
}

/// The app's own log, for reporting problems without a computer attached.
class _LogView extends StatelessWidget {
  const _LogView({required this.node});

  final HomeNode node;

  @override
  Widget build(BuildContext context) {
    return ExpansionTile(
      title: const Text('Diagnostics log'),
      children: [
        Builder(
          builder: (context) {
            final log = node.app.recentLog(lines: 200);
            return Column(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    TextButton.icon(
                      onPressed: () =>
                          Clipboard.setData(ClipboardData(text: log)),
                      icon: const Icon(Icons.copy),
                      label: const Text('Copy'),
                    ),
                    // The whole file, not just what's shown, by mail say.
                    TextButton.icon(
                      onPressed: log.isEmpty
                          ? null
                          : () => SharePlus.instance.share(
                              ShareParams(
                                files: [XFile(node.app.logFile())],
                                fileNameOverrides: ['osp-app-log.txt'],
                              ),
                            ),
                      icon: const Icon(Icons.ios_share),
                      label: const Text('Share'),
                    ),
                  ],
                ),
                Padding(
                  padding: const EdgeInsets.all(12),
                  child: SelectableText(
                    log.isEmpty ? '(empty)' : log,
                    style: const TextStyle(
                      fontFamily: 'monospace',
                      fontSize: 11,
                    ),
                  ),
                ),
              ],
            );
          },
        ),
      ],
    );
  }
}
