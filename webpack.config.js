
const { CleanWebpackPlugin } = require('clean-webpack-plugin');
const path = require('path');

// Extract CSS
const ExtractTextPlugin = require("extract-text-webpack-plugin");
const extractCSS = new ExtractTextPlugin('styles.min.css');

module.exports = {
    entry: [
        './web/js/index.js',
        './web/style.css'
    ],
    output: {
        path: path.resolve(__dirname, 'static'),
        filename: 'bundle.min.js'
    },
    plugins: [
        new CleanWebpackPlugin(),
        extractCSS
    ],
    module: {
        rules: [{
            test: /\.css$/,
            use: extractCSS.extract([
                {
                    loader: 'css-loader',
                    options: { importLoaders: 1 }
                },
                'postcss-loader'
            ])
        },
        {
            test: /\.(woff(2)?|ttf|eot|svg)(\?v=\d+\.\d+\.\d+)?$/,
            use: [
                {
                    loader: 'file-loader',
                    options: {
                        name: '[name].[ext]',
                        outputPath: 'fonts/'
                    }
                }
            ]
        },
        {
            test: /\.m?js$/,
            exclude: /(node_modules|bower_components)/,
            use: {
                loader: 'babel-loader',
                options: {
                    presets: ['@babel/preset-env']
                }
            }
        }
        ]
    }
}