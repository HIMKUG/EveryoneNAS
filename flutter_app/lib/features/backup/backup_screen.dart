import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';

/// 增量备份规则与场景化省电流控配置页（直接读写 SQLite `sync_rules` 表）。
class BackupScreen extends StatefulWidget {
  const BackupScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<BackupScreen> createState() => _BackupScreenState();
}

class _BackupScreenState extends State<BackupScreen> {
  List<SyncRuleModel> _rules = const <SyncRuleModel>[];
  bool _loadingRules = true;
  bool _savingRule = false;
  bool _savingPolicy = false;
  String? _scanningRuleId;

  final TextEditingController _ruleIdController = TextEditingController();
  final TextEditingController _localDirController = TextEditingController();
  final TextEditingController _ssidsController = TextEditingController();
  final TextEditingController _maxKbpsController = TextEditingController(text: '0');

  bool _wifiOnly = true;
  bool _requireCharging = false;
  String? _feedback;
  String? _error;

  @override
  void initState() {
    super.initState();
    _loadRules();
  }

  @override
  void dispose() {
    _ruleIdController.dispose();
    _localDirController.dispose();
    _ssidsController.dispose();
    _maxKbpsController.dispose();
    super.dispose();
  }

  Future<void> _loadRules() async {
    setState(() {
      _loadingRules = true;
      _error = null;
    });
    try {
      final rules = await widget.bridge.listSyncRules();
      if (!mounted) return;
      setState(() {
        _rules = rules;
        _loadingRules = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _loadingRules = false;
        _error = '加载同步规则失败：$err';
      });
    }
  }

  Future<void> _addRule() async {
    final ruleId = _ruleIdController.text.trim();
    final localDir = _localDirController.text.trim();
    if (ruleId.isEmpty || localDir.isEmpty) {
      setState(() => _error = '请填写规则标识与本地目录路径。');
      return;
    }
    setState(() {
      _savingRule = true;
      _error = null;
      _feedback = null;
    });
    try {
      await widget.bridge.upsertSyncRule(
        ruleId: ruleId,
        localDir: localDir,
        targetVaultId: 'default',
        enabled: true,
      );
      _ruleIdController.clear();
      _localDirController.clear();
      await _loadRules();
      if (!mounted) return;
      setState(() {
        _savingRule = false;
        _feedback = '已保存规则 $ruleId 并刷新实时目录监听器。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _savingRule = false;
        _error = '保存规则失败：$err';
      });
    }
  }

  Future<void> _triggerScan(String ruleId) async {
    setState(() {
      _scanningRuleId = ruleId;
      _error = null;
      _feedback = null;
    });
    try {
      await widget.bridge.triggerIncrementalScan(ruleId);
      if (!mounted) return;
      setState(() {
        _scanningRuleId = null;
        _feedback = '已完成规则 $ruleId 的 FastCDC 增量扫描、加密上传与快照提交。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _scanningRuleId = null;
        _error = '规则 $ruleId 扫描失败：$err';
      });
    }
  }

  Future<void> _savePolicy() async {
    final maxKbps = int.tryParse(_maxKbpsController.text.trim());
    if (maxKbps == null || maxKbps < 0) {
      setState(() => _error = '限速值必须为大于或等于 0 的整数（0 表示不限速）。');
      return;
    }

    final ssids = _ssidsController.text
        .split(',')
        .map((e) => e.trim())
        .where((e) => e.isNotEmpty)
        .toList(growable: false);

    setState(() {
      _savingPolicy = true;
      _error = null;
      _feedback = null;
    });

    try {
      await widget.bridge.updatePowerNetworkPolicy(
        wifiOnly: _wifiOnly,
        allowedSsids: ssids,
        requireCharging: _requireCharging,
        maxKbps: maxKbps,
      );
      if (!mounted) return;
      setState(() {
        _savingPolicy = false;
        _feedback = '已更新省电与令牌桶流控策略。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _savingPolicy = false;
        _error = '保存策略失败：$err';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final dirHint = Platform.isAndroid
        ? '/storage/emulated/0/DCIM'
        : r'C:\Users\YourName\Pictures';

    return ListView(
      padding: const EdgeInsets.all(NeuSpace.md),
      children: <Widget>[
        Text('增量备份与省电流控', style: textTheme.headlineSmall),
        const SizedBox(height: NeuSpace.md),
        if (_error != null) ...<Widget>[
          NeuCard(
            inset: true,
            child: Text(
              _error!,
              style: textTheme.bodyMedium?.copyWith(color: NeuTokens.danger),
            ),
          ),
          const SizedBox(height: NeuSpace.md),
        ],
        if (_feedback != null) ...<Widget>[
          NeuCard(
            inset: true,
            child: Text(
              _feedback!,
              style: textTheme.bodyMedium?.copyWith(color: NeuTokens.healthy),
            ),
          ),
          const SizedBox(height: NeuSpace.md),
        ],
        NeuCard(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('已持久化备份规则', style: textTheme.titleMedium),
              const SizedBox(height: NeuSpace.sm),
              if (_loadingRules)
                Text('正在读取本地规则...', style: textTheme.bodyMedium)
              else if (_rules.isEmpty)
                Text('暂无同步规则，请在下方添加需要备份的目录。', style: textTheme.bodyMedium)
              else
                for (final rule in _rules) ...<Widget>[
                  NeuCard(
                    inset: true,
                    radius: NeuRadius.concentric(NeuRadius.card, NeuSpace.sm),
                    padding: const EdgeInsets.all(NeuSpace.sm),
                    child: Row(
                      children: <Widget>[
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(rule.ruleId, style: textTheme.titleSmall),
                              const SizedBox(height: NeuSpace.xs),
                              Text(
                                '${rule.localDir} -> ${rule.targetVaultId}',
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: NeuSpace.sm),
                        NeuButton(
                          label: '手动扫描',
                          icon: Icons.sync_rounded,
                          loading: _scanningRuleId == rule.ruleId,
                          onPressed: _scanningRuleId == null
                              ? () => _triggerScan(rule.ruleId)
                              : null,
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: NeuSpace.sm),
                ],
              const SizedBox(height: NeuSpace.sm),
              NeuInsetField(
                controller: _ruleIdController,
                label: '新规则标识',
                hint: '例如：rule-work',
                prefixIcon: Icons.label_outline_rounded,
              ),
              const SizedBox(height: NeuSpace.sm),
              NeuInsetField(
                controller: _localDirController,
                label: '本地目录路径',
                hint: dirHint,
                prefixIcon: Icons.folder_outlined,
              ),
              const SizedBox(height: NeuSpace.sm),
              NeuButton(
                label: '保存规则并启用监听',
                icon: Icons.add_circle_outline_rounded,
                loading: _savingRule,
                onPressed: _addRule,
              ),
            ],
          ),
        ),
        const SizedBox(height: NeuSpace.md),
        NeuCard(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('场景化省电与令牌桶限速', style: textTheme.titleMedium),
              const SizedBox(height: NeuSpace.sm),
              NeuSwitch(
                title: '仅 Wi-Fi 网络下执行同步',
                subtitle: '避免消耗蜂窝流量',
                value: _wifiOnly,
                onChanged: (v) => setState(() => _wifiOnly = v),
              ),
              const SizedBox(height: NeuSpace.sm),
              NeuSwitch(
                title: '仅充电时全速扫描上传',
                subtitle: '息屏非充电场景降低耗电',
                value: _requireCharging,
                onChanged: (v) => setState(() => _requireCharging = v),
              ),
              const SizedBox(height: NeuSpace.sm),
              NeuInsetField(
                controller: _ssidsController,
                label: '允许的家庭 Wi-Fi SSID（逗号分隔，留空不限）',
                hint: 'HomeWiFi-5G, Studio-LAN',
                prefixIcon: Icons.wifi_rounded,
              ),
              const SizedBox(height: NeuSpace.md),
              NeuInsetField(
                controller: _maxKbpsController,
                label: '令牌桶上传限速（KB/s，0 为不限速）',
                hint: '0',
                keyboardType: TextInputType.number,
                prefixIcon: Icons.speed_rounded,
              ),
              const SizedBox(height: NeuSpace.md),
              NeuButton(
                label: '保存同步策略',
                icon: Icons.save_outlined,
                tone: NeuButtonTone.primary,
                loading: _savingPolicy,
                onPressed: _savePolicy,
              ),
            ],
          ),
        ),
      ],
    );
  }
}
