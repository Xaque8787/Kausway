using System;
using System.Collections.Generic;
using System.Globalization;
using Jellyfin.Plugin.Kausway.Configuration;
using MediaBrowser.Common.Configuration;
using MediaBrowser.Common.Plugins;
using MediaBrowser.Model.Plugins;
using MediaBrowser.Model.Serialization;

namespace Jellyfin.Plugin.Kausway;

/// <summary>
/// Admin plugin for a separately running kausway-host. It only talks to the host's control API and
/// never starts or stops the host process.
/// </summary>
public class Plugin : BasePlugin<PluginConfiguration>, IHasWebPages
{
    /// <summary>The plugin id. Also written to meta.json by the Docker build.</summary>
    public const string PluginId = "7c7a2311-48af-55e1-bc29-c41da361c57f";

    /// <summary>
    /// Initializes a new instance of the <see cref="Plugin"/> class.
    /// </summary>
    /// <param name="applicationPaths">Jellyfin application paths.</param>
    /// <param name="xmlSerializer">Jellyfin XML serializer.</param>
    public Plugin(IApplicationPaths applicationPaths, IXmlSerializer xmlSerializer)
        : base(applicationPaths, xmlSerializer)
    {
        Instance = this;
    }

    /// <summary>Gets the loaded plugin, set by Jellyfin when it constructs the plugin.</summary>
    public static Plugin? Instance { get; private set; }

    /// <inheritdoc />
    public override string Name => "Kausway Remote Access";

    /// <inheritdoc />
    public override string Description => "Manage private peer-to-peer remote access through kausway-host.";

    /// <inheritdoc />
    public override Guid Id => Guid.Parse(PluginId);

    /// <inheritdoc />
    public IEnumerable<PluginPageInfo> GetPages()
    {
        return
        [
            new PluginPageInfo
            {
                Name = Name,
                EmbeddedResourcePath = string.Format(
                    CultureInfo.InvariantCulture,
                    "{0}.Configuration.configPage.html",
                    GetType().Namespace),
            },
        ];
    }
}
