import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../format.dart';
import '../node.dart';

/// How this phone reaches its home node, and how the node is doing.
class StatusScreen extends StatefulWidget {
  const StatusScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<StatusScreen> createState() => _StatusScreenState();
}

class _StatusScreenState extends State<StatusScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.checkStatus();
  }

  Future<void> _forget() async {
    final go = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Forget your home node?'),
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

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Home node'),
        actions: [
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
                _Section('Home node'),
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
                  child: const Text('Forget this home node'),
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
                TextButton.icon(
                  onPressed: () => Clipboard.setData(ClipboardData(text: log)),
                  icon: const Icon(Icons.copy),
                  label: const Text('Copy'),
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
